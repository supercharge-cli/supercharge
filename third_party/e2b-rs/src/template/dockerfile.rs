//! Minimal hand-ported Dockerfile parser.
//!
//! Parses Dockerfile **content** into a [`DockerfileParseResult`] that holds the
//! base image and an ordered sequence of [`DockerfileAction`] values. This is a
//! 1:1 port of the `dockerfileParser.ts` keyword dispatch from the JavaScript
//! SDK; it does **not** use a full AST library — only the ~8 keywords that the
//! JS SDK actually handles are supported.
//!
//! **Path-vs-content detection** (the `fs.existsSync` check in JS) is a
//! Plan-5c concern. This module always receives already-loaded Dockerfile
//! content as a `&str`.

// Plan 5c is the caller of `parse_dockerfile`, `DockerfileParseResult`, and
// `DockerfileAction`. Suppress dead-code warnings until that milestone lands.
#![allow(dead_code)]

use std::collections::BTreeMap;

use crate::errors::{Error, Result};
use crate::template::ReadyCmd;
use crate::template::readycmd::wait_for_timeout;

/// The structured result of parsing a Dockerfile.
///
/// Contains the resolved base image and the ordered list of
/// [`DockerfileAction`] values derived from the Dockerfile instructions.
#[derive(Debug)]
pub(crate) struct DockerfileParseResult {
    /// The base image declared by the `FROM` instruction (e.g. `"node:20"`).
    ///
    /// Defaults to `"e2bdev/base"` when the `FROM` instruction has no
    /// arguments.
    pub(crate) base_image: String,
    /// Ordered sequence of template-builder actions derived from each
    /// Dockerfile instruction, including Docker defaults prepended at the start
    /// and optional E2B defaults appended at the end.
    pub(crate) actions: Vec<DockerfileAction>,
}

/// A single template-builder action derived from one Dockerfile instruction.
///
/// Returned inside [`DockerfileParseResult::actions`] and consumed by the
/// template builder in Plan 5c.
#[derive(Debug)]
pub(crate) enum DockerfileAction {
    /// Set the active user (`USER` instruction, or the Docker/E2B defaults).
    SetUser(String),
    /// Set the working directory (`WORKDIR` instruction, or the Docker/E2B
    /// defaults).
    SetWorkdir(String),
    /// Copy a single source path into the image (`COPY` / `ADD` instruction).
    ///
    /// One action is emitted **per source** when a `COPY` lists multiple
    /// source paths.
    Copy {
        /// Source path inside the build context.
        src: String,
        /// Destination path inside the image.
        dest: String,
        /// User override from the `--chown` flag (e.g. `"user:group"`).
        /// `--chmod` is intentionally ignored (matches JS `handleCopyInstruction`).
        user: Option<String>,
    },
    /// Run a shell command inside the image (`RUN` instruction).
    RunCmd(String),
    /// Set one or more environment variables (`ENV` / `ARG` instruction).
    ///
    /// Emitted only when at least one key was resolved; empty maps are dropped.
    SetEnvs(BTreeMap<String, String>),
    /// Set the sandbox start command and ready-check (`CMD` / `ENTRYPOINT`).
    SetStartCmd {
        /// The start command string. Exec-form JSON arrays are joined with
        /// spaces, matching the JS `handleCmdEntrypointInstruction` behaviour.
        cmd: String,
        /// Ready-check command. Always [`wait_for_timeout`]`(20_000)` (`sleep 20`).
        ready: ReadyCmd,
    },
}

/// Parse Dockerfile content into a [`DockerfileParseResult`].
///
/// # Errors
///
/// Returns [`Error::Template`] when:
/// - The Dockerfile contains more than one `FROM` instruction (multi-stage
///   builds are unsupported).
/// - The Dockerfile contains no `FROM` instruction at all.
///
/// # Default action sequence
///
/// Mirroring the JavaScript SDK, the returned `actions` list always begins
/// with `SetUser("root")` and `SetWorkdir("/")` (Docker runtime defaults),
/// followed by the per-instruction actions in source order, and ends with
/// `SetUser("user")` and/or `SetWorkdir("/home/user")` (E2B defaults) if the
/// Dockerfile did not set `USER` / `WORKDIR` itself.
pub(crate) fn parse_dockerfile(content: &str) -> Result<DockerfileParseResult> {
    let instructions = tokenize_instructions(content);

    let from_count = instructions.iter().filter(|(kw, _)| kw == "FROM").count();

    if from_count > 1 {
        return Err(Error::Template(
            "Multi-stage Dockerfiles are not supported".into(),
        ));
    }
    if from_count == 0 {
        return Err(Error::Template(
            "Dockerfile must contain a FROM instruction".into(),
        ));
    }

    // Base image is the first positional argument of the FROM instruction.
    // Default to "e2bdev/base" if FROM has no arguments (matches JS SDK).
    let base_image = instructions
        .iter()
        .find(|(kw, _)| kw == "FROM")
        .and_then(|(_, args)| split_args(args).into_iter().next())
        .unwrap_or_else(|| "e2bdev/base".to_string());

    let mut actions: Vec<DockerfileAction> = Vec::new();
    let mut user_changed = false;
    let mut workdir_changed = false;

    // Emit Docker runtime defaults first, mirroring the JS:
    //   templateBuilder.setUser('root')
    //   templateBuilder.setWorkdir('/')
    actions.push(DockerfileAction::SetUser("root".into()));
    actions.push(DockerfileAction::SetWorkdir("/".into()));

    for (keyword, arg_text) in &instructions {
        match keyword.as_str() {
            "FROM" => {
                // Already handled above; skip in the main loop.
            }
            "RUN" => {
                let cmd = arg_text.trim().to_string();
                if !cmd.is_empty() {
                    actions.push(DockerfileAction::RunCmd(cmd));
                }
            }
            "COPY" | "ADD" => {
                handle_copy(arg_text, &mut actions);
            }
            "WORKDIR" => {
                if let Some(path) = split_args(arg_text).into_iter().next() {
                    actions.push(DockerfileAction::SetWorkdir(path));
                    workdir_changed = true;
                }
            }
            "USER" => {
                if let Some(user) = split_args(arg_text).into_iter().next() {
                    actions.push(DockerfileAction::SetUser(user));
                    user_changed = true;
                }
            }
            "ENV" | "ARG" => {
                let envs = handle_env(arg_text, keyword);
                if !envs.is_empty() {
                    actions.push(DockerfileAction::SetEnvs(envs));
                }
            }
            "EXPOSE" | "VOLUME" => {
                // Not supported in the E2B SDK; silently ignored.
            }
            "CMD" | "ENTRYPOINT" => {
                handle_cmd_entrypoint(arg_text, &mut actions);
            }
            _ => {
                // Unknown keyword — silently skip (JS warns via console.warn).
            }
        }
    }

    // Append E2B defaults if the Dockerfile did not set USER / WORKDIR.
    if !user_changed {
        actions.push(DockerfileAction::SetUser("user".into()));
    }
    if !workdir_changed {
        actions.push(DockerfileAction::SetWorkdir("/home/user".into()));
    }

    Ok(DockerfileParseResult {
        base_image,
        actions,
    })
}

// ─────────────────────────────── internals ───────────────────────────────────

/// Tokenize raw Dockerfile content into `(KEYWORD, arg_text)` pairs.
///
/// Handles:
/// - Line continuations: a line ending in `\` is joined with the next line.
/// - Blank lines and `#`-comment lines are skipped.
/// - Keywords are returned **uppercased**; arg text preserves original case.
fn tokenize_instructions(content: &str) -> Vec<(String, String)> {
    // Phase 1 — join continuation lines.
    let mut joined: Vec<String> = Vec::new();
    let mut pending = String::new();

    for line in content.lines() {
        let trimmed_end = line.trim_end();
        if let Some(without_bs) = trimmed_end.strip_suffix('\\') {
            // Strip trailing backslash and collapse into the pending buffer.
            pending.push_str(without_bs);
            pending.push(' ');
        } else {
            pending.push_str(line);
            joined.push(std::mem::take(&mut pending));
        }
    }
    // Flush any leftover pending text (file without a trailing newline).
    if !pending.is_empty() {
        joined.push(pending);
    }

    // Phase 2 — extract keyword + arg_text pairs.
    let mut result = Vec::new();
    for line in joined {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((kw, rest)) = trimmed.split_once(|c: char| c.is_ascii_whitespace()) {
            result.push((kw.to_ascii_uppercase(), rest.to_string()));
        } else {
            // Keyword only, no arguments (e.g. bare `VOLUME` with no arg).
            result.push((trimmed.to_ascii_uppercase(), String::new()));
        }
    }
    result
}

/// Split an argument string into tokens, respecting single- and double-quoted
/// regions.
///
/// - Single-quoted spans (`'…'`) are passed through verbatim (no escaping).
/// - Double-quoted spans (`"…"`) honour `\x` escape sequences (the escaped
///   character is included literally).
/// - Tokens are delimited by ASCII whitespace outside of any quote span.
fn split_args(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '\\' if in_double => {
                // Escape sequence inside double-quoted region.
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                }
            }
            c if c.is_ascii_whitespace() && !in_single && !in_double => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Partition a token list into `(flags, positional_args)`.
///
/// Tokens that begin with `--` are treated as flags. A flag may carry a value
/// (`--name=value`) or be bare (`--flag`).
fn extract_flags(tokens: Vec<String>) -> (Vec<(String, Option<String>)>, Vec<String>) {
    let mut flags = Vec::new();
    let mut positional = Vec::new();

    for tok in tokens {
        if let Some(body) = tok.strip_prefix("--") {
            if let Some((name, value)) = body.split_once('=') {
                flags.push((name.to_string(), Some(value.to_string())));
            } else {
                flags.push((body.to_string(), None));
            }
        } else {
            positional.push(tok);
        }
    }
    (flags, positional)
}

/// Emit [`DockerfileAction::Copy`] actions for a `COPY` / `ADD` instruction.
///
/// One action is emitted per source path. Only the `--chown` flag is read;
/// `--chmod` is intentionally ignored to match the JS `handleCopyInstruction`.
fn handle_copy(arg_text: &str, actions: &mut Vec<DockerfileAction>) {
    let all_tokens = split_args(arg_text);
    let (flags, positional) = extract_flags(all_tokens);

    if positional.len() < 2 {
        return;
    }

    // Safety: guarded by `positional.len() < 2` above.
    let Some(dest) = positional.last().cloned() else {
        return;
    };
    let sources = &positional[..positional.len() - 1];

    // Only --chown is consumed; --chmod is ignored (matches JS handleCopyInstruction).
    let user: Option<String> = flags
        .iter()
        .find(|(name, _)| name == "chown")
        .and_then(|(_, val)| val.clone());

    for src in sources {
        actions.push(DockerfileAction::Copy {
            src: src.clone(),
            dest: dest.clone(),
            user: user.clone(),
        });
    }
}

/// Build a `BTreeMap<String, String>` from an `ENV` or `ARG` instruction.
///
/// Ports the JS `handleEnvInstruction` branching logic exactly:
///
/// | Argument count | Example | Behaviour |
/// |---|---|---|
/// | 1 | `ENV K=V` | split on first `=` |
/// | 1 (no `=`) | `ARG K` | `K → ""` (ARG only) |
/// | 2, both contain `=` | `ENV K1=v1 K2=v2` | split each on first `=` |
/// | 2, not both `=` | `ENV K V` | traditional `key value` format |
/// | > 2 | (line-continuation) | split each on first `=`; bare ARG keys → `""` |
fn handle_env(arg_text: &str, keyword: &str) -> BTreeMap<String, String> {
    let args = split_args(arg_text);
    let mut env_vars: BTreeMap<String, String> = BTreeMap::new();

    match args.len() {
        0 => {}

        1 => {
            let s = &args[0];
            if let Some(eq) = s.find('=') {
                if eq > 0 {
                    env_vars.insert(s[..eq].to_string(), s[eq + 1..].to_string());
                }
            } else if keyword == "ARG" && !s.trim().is_empty() {
                env_vars.insert(s.trim().to_string(), String::new());
            }
        }

        2 => {
            let first = &args[0];
            let second = &args[1];

            if first.contains('=') && second.contains('=') {
                // Both are `key=value` pairs (e.g. from a line-continuation ENV).
                for a in &args {
                    if let Some(eq) = a.find('=')
                        && eq > 0
                    {
                        env_vars.insert(a[..eq].to_string(), a[eq + 1..].to_string());
                    }
                }
            } else {
                // Traditional `ENV key value` format.
                env_vars.insert(first.clone(), second.clone());
            }
        }

        _ => {
            // Multiple arguments from line-continuation backslash.
            for a in &args {
                if let Some(eq) = a.find('=') {
                    if eq > 0 {
                        env_vars.insert(a[..eq].to_string(), a[eq + 1..].to_string());
                    }
                } else if keyword == "ARG" {
                    env_vars.insert(a.clone(), String::new());
                }
            }
        }
    }

    env_vars
}

/// Emit a [`DockerfileAction::SetStartCmd`] for a `CMD` / `ENTRYPOINT`
/// instruction.
///
/// Exec form (`["cmd", "arg"]`) is detected by attempting
/// `serde_json::from_str::<Vec<String>>` on the raw arg text; on success the
/// array elements are joined with a space. Shell form is used as-is.
/// `ready` is always [`wait_for_timeout`]`(20_000)`.
fn handle_cmd_entrypoint(arg_text: &str, actions: &mut Vec<DockerfileAction>) {
    let text = arg_text.trim();
    if text.is_empty() {
        return;
    }

    // Exec form detection: try to parse the entire arg text as a JSON string
    // array, matching the JS:
    //   JSON.parse(argumentsData.map(arg => arg.getValue()).join(' '))
    let cmd = if let Ok(parts) = serde_json::from_str::<Vec<String>>(text) {
        parts.join(" ")
    } else {
        text.to_string()
    };

    actions.push(DockerfileAction::SetStartCmd {
        cmd,
        ready: wait_for_timeout(20_000),
    });
}

// ─────────────────────────────────── tests ────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn collect_users(result: &DockerfileParseResult) -> Vec<&str> {
        result
            .actions
            .iter()
            .filter_map(|a| {
                if let DockerfileAction::SetUser(u) = a {
                    Some(u.as_str())
                } else {
                    None
                }
            })
            .collect()
    }

    fn collect_workdirs(result: &DockerfileParseResult) -> Vec<&str> {
        result
            .actions
            .iter()
            .filter_map(|a| {
                if let DockerfileAction::SetWorkdir(p) = a {
                    Some(p.as_str())
                } else {
                    None
                }
            })
            .collect()
    }

    // ── core parse tests ─────────────────────────────────────────────────────

    /// A representative Dockerfile exercises FROM / RUN / COPY / WORKDIR /
    /// USER / ENV in a single pass.
    #[test]
    fn parses_from_run_copy_workdir_user_env() {
        let content = "\
FROM node:20
RUN npm i
COPY . /app
WORKDIR /app
USER me
ENV K=V
";
        let result = parse_dockerfile(content).unwrap();
        assert_eq!(result.base_image, "node:20");

        // Ordered action assertions.
        let mut iter = result.actions.iter();

        // Docker defaults prepended first.
        assert!(matches!(iter.next(), Some(DockerfileAction::SetUser(u)) if u == "root"));
        assert!(matches!(iter.next(), Some(DockerfileAction::SetWorkdir(p)) if p == "/"));

        // RUN npm i
        assert!(matches!(iter.next(), Some(DockerfileAction::RunCmd(c)) if c == "npm i"));

        // COPY . /app  (no --chown)
        assert!(matches!(
            iter.next(),
            Some(DockerfileAction::Copy { src, dest, user })
                if src == "." && dest == "/app" && user.is_none()
        ));

        // WORKDIR /app
        assert!(matches!(iter.next(), Some(DockerfileAction::SetWorkdir(p)) if p == "/app"));

        // USER me
        assert!(matches!(iter.next(), Some(DockerfileAction::SetUser(u)) if u == "me"));

        // ENV K=V
        let Some(DockerfileAction::SetEnvs(map)) = iter.next() else {
            panic!("expected SetEnvs");
        };
        assert_eq!(map.get("K").map(String::as_str), Some("V"));

        // Both USER and WORKDIR were set, so no trailing E2B defaults.
        assert!(iter.next().is_none());
    }

    // ── error cases ──────────────────────────────────────────────────────────

    #[test]
    fn rejects_multistage() {
        let content = "FROM node:20\nFROM ubuntu\n";
        let err = parse_dockerfile(content).unwrap_err();
        assert!(
            matches!(&err, Error::Template(m) if m.contains("Multi-stage")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_missing_from() {
        let content = "RUN npm install\n";
        let err = parse_dockerfile(content).unwrap_err();
        assert!(
            matches!(&err, Error::Template(m) if m.contains("FROM")),
            "unexpected error: {err}"
        );
    }

    // ── COPY flag handling ────────────────────────────────────────────────────

    #[test]
    fn copy_with_chown_flag() {
        let content = "FROM scratch\nCOPY --chown=me a b\n";
        let result = parse_dockerfile(content).unwrap();

        let copy_action = result
            .actions
            .iter()
            .find(|a| matches!(a, DockerfileAction::Copy { .. }));
        let Some(DockerfileAction::Copy { src, dest, user }) = copy_action else {
            panic!("expected a Copy action");
        };
        assert_eq!(src, "a");
        assert_eq!(dest, "b");
        assert_eq!(user.as_deref(), Some("me"));
    }

    /// `--chmod` is ignored (matches JS handleCopyInstruction which only reads chown).
    #[test]
    fn copy_ignores_chmod_flag() {
        let content = "FROM scratch\nCOPY --chown=me:me --chmod=755 a b\n";
        let result = parse_dockerfile(content).unwrap();

        let Some(DockerfileAction::Copy { src, dest, user }) = result
            .actions
            .iter()
            .find(|a| matches!(a, DockerfileAction::Copy { .. }))
        else {
            panic!("expected a Copy action");
        };
        assert_eq!(src, "a");
        assert_eq!(dest, "b");
        assert_eq!(user.as_deref(), Some("me:me")); // chown captured
        // chmod is intentionally not in the result
    }

    /// Multiple sources produce one Copy action per source.
    #[test]
    fn copy_multiple_sources() {
        let content = "FROM scratch\nCOPY a b c /dest\n";
        let result = parse_dockerfile(content).unwrap();

        let copies: Vec<_> = result
            .actions
            .iter()
            .filter_map(|a| {
                if let DockerfileAction::Copy { src, dest, .. } = a {
                    Some((src.as_str(), dest.as_str()))
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(copies, [("a", "/dest"), ("b", "/dest"), ("c", "/dest")]);
    }

    // ── CMD / ENTRYPOINT ─────────────────────────────────────────────────────

    #[test]
    fn cmd_exec_and_shell_form() {
        // Exec form: JSON array → join elements.
        let exec_result = parse_dockerfile("FROM scratch\nCMD [\"npm\", \"start\"]\n").unwrap();
        let Some(DockerfileAction::SetStartCmd { cmd, ready }) = exec_result
            .actions
            .iter()
            .find(|a| matches!(a, DockerfileAction::SetStartCmd { .. }))
        else {
            panic!("expected SetStartCmd (exec form)");
        };
        assert_eq!(cmd, "npm start");
        assert_eq!(ready.cmd(), "sleep 20");

        // Shell form: raw text used as-is.
        let shell_result = parse_dockerfile("FROM scratch\nCMD npm start\n").unwrap();
        let Some(DockerfileAction::SetStartCmd { cmd: shell_cmd, .. }) = shell_result
            .actions
            .iter()
            .find(|a| matches!(a, DockerfileAction::SetStartCmd { .. }))
        else {
            panic!("expected SetStartCmd (shell form)");
        };
        assert_eq!(shell_cmd, "npm start");
    }

    #[test]
    fn entrypoint_exec_form() {
        let content = "FROM scratch\nENTRYPOINT [\"node\", \"server.js\"]\n";
        let result = parse_dockerfile(content).unwrap();
        let Some(DockerfileAction::SetStartCmd { cmd, .. }) = result
            .actions
            .iter()
            .find(|a| matches!(a, DockerfileAction::SetStartCmd { .. }))
        else {
            panic!("expected SetStartCmd");
        };
        assert_eq!(cmd, "node server.js");
    }

    // ── ignored instructions ─────────────────────────────────────────────────

    #[test]
    fn ignores_expose_volume() {
        let content = "\
FROM scratch
EXPOSE 8080
VOLUME /data
";
        let result = parse_dockerfile(content).unwrap();

        // Only the Docker-default and E2B-default SetUser / SetWorkdir should appear.
        for action in &result.actions {
            assert!(
                matches!(
                    action,
                    DockerfileAction::SetUser(_) | DockerfileAction::SetWorkdir(_)
                ),
                "unexpected action: {action:?}"
            );
        }
    }

    // ── E2B defaults ─────────────────────────────────────────────────────────

    #[test]
    fn applies_e2b_defaults_when_no_user_or_workdir() {
        let content = "FROM scratch\nRUN echo hi\n";
        let result = parse_dockerfile(content).unwrap();

        let users = collect_users(&result);
        let workdirs = collect_workdirs(&result);

        // Trailing E2B defaults must be appended.
        assert_eq!(users.last().copied(), Some("user"));
        assert_eq!(workdirs.last().copied(), Some("/home/user"));
    }

    #[test]
    fn no_e2b_user_default_when_user_instruction_present() {
        let content = "FROM scratch\nUSER myuser\n";
        let result = parse_dockerfile(content).unwrap();

        let users = collect_users(&result);
        // The trailing "user" default must NOT appear.
        assert_ne!(users.last().copied(), Some("user"));
        // The USER instruction itself should be present.
        assert!(users.contains(&"myuser"));
    }

    #[test]
    fn no_e2b_workdir_default_when_workdir_instruction_present() {
        let content = "FROM scratch\nWORKDIR /app\n";
        let result = parse_dockerfile(content).unwrap();

        let workdirs = collect_workdirs(&result);
        // The trailing "/home/user" default must NOT appear.
        assert_ne!(workdirs.last().copied(), Some("/home/user"));
        assert!(workdirs.contains(&"/app"));
    }

    // ── ENV / ARG forms ───────────────────────────────────────────────────────

    #[test]
    fn env_forms() {
        // 1. ENV K=V  (single arg, contains '=')
        {
            let r = parse_dockerfile("FROM scratch\nENV K=V\n").unwrap();
            let Some(DockerfileAction::SetEnvs(map)) = r
                .actions
                .iter()
                .find(|a| matches!(a, DockerfileAction::SetEnvs(_)))
            else {
                panic!("expected SetEnvs for ENV K=V");
            };
            assert_eq!(map.get("K").map(String::as_str), Some("V"));
        }

        // 2. ENV K V  (two args, no '=')
        {
            let r = parse_dockerfile("FROM scratch\nENV K V\n").unwrap();
            let Some(DockerfileAction::SetEnvs(map)) = r
                .actions
                .iter()
                .find(|a| matches!(a, DockerfileAction::SetEnvs(_)))
            else {
                panic!("expected SetEnvs for ENV K V");
            };
            assert_eq!(map.get("K").map(String::as_str), Some("V"));
        }

        // 3. ENV K1=v1 K2=v2  (two args, both contain '=')
        {
            let r = parse_dockerfile("FROM scratch\nENV K1=v1 K2=v2\n").unwrap();
            let Some(DockerfileAction::SetEnvs(map)) = r
                .actions
                .iter()
                .find(|a| matches!(a, DockerfileAction::SetEnvs(_)))
            else {
                panic!("expected SetEnvs for ENV K1=v1 K2=v2");
            };
            assert_eq!(map.get("K1").map(String::as_str), Some("v1"));
            assert_eq!(map.get("K2").map(String::as_str), Some("v2"));
        }

        // 4. ARG K  (no default) → key with empty value
        {
            let r = parse_dockerfile("FROM scratch\nARG K\n").unwrap();
            let Some(DockerfileAction::SetEnvs(map)) = r
                .actions
                .iter()
                .find(|a| matches!(a, DockerfileAction::SetEnvs(_)))
            else {
                panic!("expected SetEnvs for ARG K");
            };
            assert_eq!(map.get("K").map(String::as_str), Some(""));
        }
    }

    // ── line continuation ─────────────────────────────────────────────────────

    #[test]
    fn line_continuation_in_run() {
        let content = "\
FROM scratch
RUN apt-get update && \\
    apt-get install -y curl
";
        let result = parse_dockerfile(content).unwrap();
        let Some(DockerfileAction::RunCmd(cmd)) = result
            .actions
            .iter()
            .find(|a| matches!(a, DockerfileAction::RunCmd(_)))
        else {
            panic!("expected RunCmd");
        };
        assert!(cmd.contains("apt-get update"), "cmd: {cmd}");
        assert!(cmd.contains("apt-get install"), "cmd: {cmd}");
    }

    // ── base-image fallback ───────────────────────────────────────────────────

    #[test]
    fn from_with_no_args_defaults_to_e2bdev_base() {
        // An unusual but valid Dockerfile (FROM with no arg).
        let content = "FROM\nRUN true\n";
        // The line "FROM\nRUN..." — "FROM" has no args so fallback applies.
        // Actually since our tokenizer splits on whitespace the keyword "FROM"
        // will have arg_text "" → split_args returns [] → fallback.
        // This is a rare edge case; we just verify it doesn't panic/err.
        let result = parse_dockerfile(content).unwrap();
        assert_eq!(result.base_image, "e2bdev/base");
    }
}
