//! Pure git command-building, credential, and output-parsing helpers.

use super::types::{GitBranches, GitFileStatus, GitStatus, GitStatusLabel};
use crate::errors::{Error, Result};
use crate::sandbox::commands::CommandResult;

// ---------------------------------------------------------------------------
// Command building
// ---------------------------------------------------------------------------

/// Build a shell command string: `git [-C <repo_path>] <args…>`, with every
/// token shell-quoted so it is safe to pass to `sh -c`.
pub(crate) fn build_git_command(args: &[&str], repo_path: Option<&str>) -> String {
    let mut parts: Vec<String> = vec!["git".to_string()];
    if let Some(repo) = repo_path {
        parts.push("-C".to_string());
        parts.push(repo.to_string());
    }
    parts.extend(args.iter().map(|a| (*a).to_string()));
    parts
        .iter()
        .map(|p| crate::utils::shell_quote(p))
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Auth / upstream failure detection
// ---------------------------------------------------------------------------

const AUTH_SNIPPETS: &[&str] = &[
    "authentication failed",
    "terminal prompts disabled",
    "could not read username",
    "invalid username or password",
    "access denied",
    "permission denied",
    "not authorized",
];

const UPSTREAM_SNIPPETS: &[&str] = &[
    "has no upstream branch",
    "no upstream branch",
    "no upstream configured",
    "no tracking information for the current branch",
    "no tracking information",
    "set the remote as upstream",
    "set the upstream branch",
    "please specify which branch you want to merge with",
];

fn output_matches(result: &CommandResult, snippets: &[&str]) -> bool {
    let haystack = format!("{}\n{}", result.stderr, result.stdout).to_lowercase();
    snippets.iter().any(|s| haystack.contains(s))
}

/// Return `true` when a failed `git` command's stderr/stdout indicate an
/// authentication failure (wrong credentials, access denied, etc.).
pub(crate) fn is_auth_failure(result: &CommandResult) -> bool {
    output_matches(result, AUTH_SNIPPETS)
}

/// Return `true` when a failed `git` command's output indicates the current
/// branch has no configured upstream tracking branch.
pub(crate) fn is_missing_upstream(result: &CommandResult) -> bool {
    output_matches(result, UPSTREAM_SNIPPETS)
}

// ---------------------------------------------------------------------------
// Credential helpers
// ---------------------------------------------------------------------------

/// Embed `user:pass` into an HTTP(S) git URL.
///
/// Returns the URL unchanged when both `username` and `password` are `None`.
/// Returns [`Error::InvalidArgument`] if exactly one of the two is provided,
/// or if the URL uses a non-HTTP(S) scheme.
pub(crate) fn with_credentials(
    url: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<String> {
    match (username, password) {
        (None, None) => Ok(url.to_string()),
        (Some(_), None) | (None, Some(_)) => Err(Error::InvalidArgument(
            "Both username and password are required when using Git credentials.".to_string(),
        )),
        (Some(user), Some(pass)) => {
            let mut parsed = reqwest::Url::parse(url)
                .map_err(|_| Error::InvalidArgument(format!("Invalid Git URL: {url}")))?;
            if parsed.scheme() != "http" && parsed.scheme() != "https" {
                return Err(Error::InvalidArgument(
                    "Only http(s) Git URLs support username/password credentials.".to_string(),
                ));
            }
            parsed
                .set_username(user)
                .map_err(|()| Error::InvalidArgument("Invalid Git URL host".to_string()))?;
            parsed
                .set_password(Some(pass))
                .map_err(|()| Error::InvalidArgument("Invalid Git URL host".to_string()))?;
            Ok(parsed.to_string())
        }
    }
}

/// Remove any embedded `user:pass` from an HTTP(S) URL.
///
/// If the URL cannot be parsed, or is not HTTP(S), it is returned unchanged
/// (safe fallback that never panics).
pub(crate) fn strip_credentials(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut parsed) => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.to_string()
        }
        Err(_) => url.to_string(),
    }
}

/// Derive the default clone-destination directory name from a git URL.
///
/// Returns the last non-empty path segment with any `.git` suffix stripped,
/// or `None` if the URL cannot be parsed or has no meaningful last segment.
pub(crate) fn derive_repo_dir_from_url(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let trimmed = parsed.path().trim_end_matches('/');
    let last = trimmed.rsplit('/').next()?;
    if last.is_empty() {
        return None;
    }
    Some(last.strip_suffix(".git").unwrap_or(last).to_string())
}

// ---------------------------------------------------------------------------
// Output parsers — private helpers
// ---------------------------------------------------------------------------

/// Parse the `[ahead N, behind M]` segment from a `git status --porcelain=1 -b`
/// branch line into `(ahead, behind)` commit counts.
fn parse_ahead_behind(segment: Option<&str>) -> (i32, i32) {
    let Some(seg) = segment else {
        return (0, 0);
    };

    let ahead = if seg.contains("ahead") {
        seg.split("ahead")
            .nth(1)
            .and_then(|s| s.split(',').next())
            .and_then(|s| s.trim().parse::<i32>().ok())
            .unwrap_or(0)
    } else {
        0
    };

    let behind = if seg.contains("behind") {
        seg.split("behind")
            .nth(1)
            .and_then(|s| s.split(',').next())
            .and_then(|s| s.trim().parse::<i32>().ok())
            .unwrap_or(0)
    } else {
        0
    };

    (ahead, behind)
}

/// Normalise a raw branch name from porcelain output into a plain branch name.
///
/// Handles detached HEAD notation, "no branch", and "no commits yet" prefixes.
fn normalize_branch_name(name: &str) -> String {
    if name.starts_with("HEAD (detached at ") {
        return name
            .strip_prefix("HEAD (detached at ")
            .unwrap_or(name)
            .trim_end_matches(')')
            .to_string();
    }
    name.replace("HEAD (no branch)", "HEAD")
        .replace("No commits yet on ", "")
        .replace("Initial commit on ", "")
}

/// Derive the highest-priority [`GitStatusLabel`] from a (index, worktree) pair
/// using the same priority order as the JS SDK.
fn derive_status(index_status: char, working_status: char) -> GitStatusLabel {
    let chars = [index_status, working_status];
    if chars.contains(&'U') {
        return GitStatusLabel::Conflict;
    }
    if chars.contains(&'R') {
        return GitStatusLabel::Renamed;
    }
    if chars.contains(&'C') {
        return GitStatusLabel::Copied;
    }
    if chars.contains(&'D') {
        return GitStatusLabel::Deleted;
    }
    if chars.contains(&'A') {
        return GitStatusLabel::Added;
    }
    if chars.contains(&'M') {
        return GitStatusLabel::Modified;
    }
    if chars.contains(&'T') {
        return GitStatusLabel::TypeChange;
    }
    if chars.contains(&'?') {
        return GitStatusLabel::Untracked;
    }
    GitStatusLabel::Unknown
}

// ---------------------------------------------------------------------------
// Output parsers — public(crate)
// ---------------------------------------------------------------------------

/// Parse `git status --porcelain=1 -b` output into a structured [`GitStatus`].
///
/// This is a faithful port of `parseGitStatus` from the JS SDK (`git/utils.ts`).
pub(crate) fn parse_git_status(output: &str) -> GitStatus {
    // Split on newlines, strip trailing CR (Windows line endings), skip blanks.
    let lines: Vec<&str> = output
        .split('\n')
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty())
        .collect();

    let mut current_branch: Option<String> = None;
    let mut upstream: Option<String> = None;
    let mut ahead: i32 = 0;
    let mut behind: i32 = 0;
    let mut detached = false;
    let mut file_status: Vec<GitFileStatus> = Vec::new();

    if lines.is_empty() {
        return GitStatus {
            current_branch,
            upstream,
            ahead,
            behind,
            detached,
            file_status,
            is_clean: true,
            has_changes: false,
            has_staged: false,
            has_untracked: false,
            has_conflicts: false,
            total_count: 0,
            staged_count: 0,
            unstaged_count: 0,
            untracked_count: 0,
            conflict_count: 0,
        };
    }

    // --- Branch / tracking line ---
    let branch_line = lines[0];
    if let Some(branch_info) = branch_line.strip_prefix("## ") {
        let ahead_start = branch_info.find(" [");
        let branch_part = match ahead_start {
            None => branch_info,
            Some(idx) => &branch_info[..idx],
        };
        let ahead_part: Option<&str> = match ahead_start {
            None => None,
            Some(idx) => {
                // slice(aheadStart + 2, -1) → skip "[ " prefix and drop trailing "]"
                let rest = &branch_info[idx + 2..];
                Some(rest.strip_suffix(']').unwrap_or(rest))
            }
        };

        let normalized = normalize_branch_name(branch_part);
        let raw = branch_part;
        let is_detached = raw.starts_with("HEAD (detached at ") || raw.contains("detached");

        if is_detached || normalized.starts_with("HEAD") {
            detached = true;
        } else if normalized.contains("...") {
            // "branch...upstream"
            let mut parts = normalized.splitn(2, "...");
            current_branch = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
            upstream = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
        } else {
            current_branch = if normalized.is_empty() {
                None
            } else {
                Some(normalized)
            };
        }

        let (a, b) = parse_ahead_behind(ahead_part);
        ahead = a;
        behind = b;
    }

    // --- File status lines ---
    for line in &lines[1..] {
        // Untracked files: "?? <path>"
        if let Some(name) = line.strip_prefix("?? ") {
            file_status.push(GitFileStatus {
                name: name.to_string(),
                status: GitStatusLabel::Untracked,
                index_status: '?',
                working_tree_status: '?',
                staged: false,
                renamed_from: None,
            });
            continue;
        }

        // Need at least "XY " (3 chars) to be a valid status line.
        if line.len() < 3 {
            continue;
        }

        // First two chars are ASCII status codes; byte-index is safe here.
        let index_status = char::from(line.as_bytes()[0]);
        let working_tree_status = char::from(line.as_bytes()[1]);
        let path = &line[3..];

        // Rename/copy detection: "old -> new"
        let (name, renamed_from) = if path.contains(" -> ") {
            // Split on the FIRST " -> " only; the new name may itself contain " -> ".
            let mut parts = path.splitn(2, " -> ");
            let from = parts.next().unwrap_or("").to_string();
            let to = parts.next().unwrap_or("").to_string();
            (to, Some(from))
        } else {
            (path.to_string(), None)
        };

        // A file is staged when the index code is not ' ' or '?'.
        let staged = index_status != ' ' && index_status != '?';

        file_status.push(GitFileStatus {
            name,
            status: derive_status(index_status, working_tree_status),
            index_status,
            working_tree_status,
            staged,
            renamed_from,
        });
    }

    // --- Aggregate counts ---
    let total_count = file_status.len();
    let staged_count = file_status.iter().filter(|f| f.staged).count();
    let untracked_count = file_status
        .iter()
        .filter(|f| matches!(f.status, GitStatusLabel::Untracked))
        .count();
    let conflict_count = file_status
        .iter()
        .filter(|f| matches!(f.status, GitStatusLabel::Conflict))
        .count();
    let unstaged_count = total_count - staged_count;

    GitStatus {
        current_branch,
        upstream,
        ahead,
        behind,
        detached,
        file_status,
        is_clean: total_count == 0,
        has_changes: total_count > 0,
        has_staged: staged_count > 0,
        has_untracked: untracked_count > 0,
        has_conflicts: conflict_count > 0,
        total_count,
        staged_count,
        unstaged_count,
        untracked_count,
        conflict_count,
    }
}

/// Parse `git branch --format=%(refname:short)\t%(HEAD)` output into a
/// structured [`GitBranches`].
///
/// Each non-empty, trimmed line is expected to be `<name>\t<marker>` where
/// `<marker>` is `*` for the current branch or empty otherwise.
pub(crate) fn parse_git_branches(output: &str) -> GitBranches {
    let mut branches: Vec<String> = Vec::new();
    let mut current_branch: Option<String> = None;

    for line in output.split('\n').map(str::trim).filter(|l| !l.is_empty()) {
        let mut parts = line.splitn(2, '\t');
        let name = match parts.next() {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => continue,
        };
        if parts.next() == Some("*") {
            current_branch = Some(name.clone());
        }
        branches.push(name);
    }

    GitBranches {
        branches,
        current_branch,
    }
}

// ---------------------------------------------------------------------------
// Push helpers and error messages
// ---------------------------------------------------------------------------

/// Build the argument list for `git push`.
///
/// Port of `buildPushArgs` from `git/utils.ts`. `remote_name` is used in the
/// credentialed path (already resolved via [`crate::sandbox::git::Git`]'s
/// private `resolve_remote_name`); `remote` is the raw caller option used in
/// the non-credentialed path. `--set-upstream` is added when `set_upstream`
/// is `true` and a remote target is known.
pub(crate) fn build_push_args(
    remote_name: Option<&str>,
    remote: Option<&str>,
    branch: Option<&str>,
    set_upstream: bool,
) -> Vec<String> {
    let mut args = vec!["push".to_string()];
    let target_remote = remote_name.or(remote);
    if set_upstream && target_remote.is_some() {
        args.push("--set-upstream".to_string());
    }
    if let Some(r) = target_remote {
        args.push(r.to_string());
    }
    if let Some(b) = branch {
        args.push(b.to_string());
    }
    args
}

/// Build a human-readable error message for a git authentication failure.
///
/// Port of `buildAuthErrorMessage` from `git/utils.ts`.
///
/// Set `missing_password` to `true` when a username was supplied but no
/// password — indicating the caller knows credentials are required but only
/// provided half of them.
pub(crate) fn build_auth_error_message(action: &str, missing_password: bool) -> String {
    if missing_password {
        format!("Git {action} requires a password/token for private repositories.")
    } else {
        format!("Git {action} requires credentials for private repositories.")
    }
}

/// Build a human-readable error message for a missing upstream tracking branch.
///
/// Port of `buildUpstreamErrorMessage` from `git/utils.ts`.
pub(crate) fn build_upstream_error_message(action: &str) -> String {
    if action == "push" {
        "Git push failed because no upstream branch is configured. \
         Set upstream once with { setUpstream: true } (and optional remote/branch), \
         or pass remote and branch explicitly."
            .to_string()
    } else {
        "Git pull failed because no upstream branch is configured. \
         Pass remote and branch explicitly, or set upstream once \
         (push with { setUpstream: true } or run: \
         git branch --set-upstream-to=origin/<branch> <branch>)."
            .to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_command_with_repo_and_quotes() {
        assert_eq!(build_git_command(&["status"], None), "git status");
        assert_eq!(
            build_git_command(&["commit", "-m", "a b"], Some("/repo")),
            "git -C /repo commit -m 'a b'"
        );
    }

    #[test]
    fn credentials_validate_and_inject() {
        assert_eq!(
            with_credentials("https://h/r.git", None, None).expect("noop"),
            "https://h/r.git"
        );
        assert!(with_credentials("https://h/r.git", Some("u"), None).is_err()); // one-of
        assert!(with_credentials("ftp://h/r", Some("u"), Some("p")).is_err()); // non-http(s)
        let u = with_credentials("https://h/r.git", Some("u"), Some("p")).expect("creds");
        assert!(u.starts_with("https://u:p@h"));
    }

    #[test]
    fn derives_repo_dir() {
        assert_eq!(
            derive_repo_dir_from_url("https://h/owner/repo.git").as_deref(),
            Some("repo")
        );
        assert_eq!(
            derive_repo_dir_from_url("https://h/owner/repo/").as_deref(),
            Some("repo")
        );
        assert_eq!(derive_repo_dir_from_url("not a url"), None);
    }

    #[test]
    fn detects_auth_and_upstream() {
        let auth = CommandResult {
            exit_code: 128,
            error: None,
            stdout: String::new(),
            stderr: "fatal: Authentication failed for 'x'".to_string(),
        };
        assert!(is_auth_failure(&auth));
        let up = CommandResult {
            exit_code: 128,
            error: None,
            stdout: String::new(),
            stderr: "fatal: The current branch has no upstream branch.".to_string(),
        };
        assert!(is_missing_upstream(&up));
        let ok = CommandResult {
            exit_code: 0,
            error: None,
            stdout: String::new(),
            stderr: String::new(),
        };
        assert!(!is_auth_failure(&ok) && !is_missing_upstream(&ok));
    }

    #[test]
    fn parses_status_branch_ahead_behind_and_files() {
        let out =
            "## main...origin/main [ahead 1, behind 2]\n M src/a.rs\n?? new.txt\nA  staged.txt\n";
        let s = parse_git_status(out);
        assert_eq!(s.current_branch.as_deref(), Some("main"));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!(s.ahead, 1);
        assert_eq!(s.behind, 2);
        assert_eq!(s.file_status.len(), 3);
        assert!(s.has_untracked);
        assert!(s.has_staged);
        assert!(!s.is_clean);
    }

    #[test]
    fn parses_clean_status() {
        let s = parse_git_status("## main\n");
        assert!(s.is_clean);
        assert_eq!(s.file_status.len(), 0);
    }

    #[test]
    fn parses_branches_with_current() {
        let b = parse_git_branches("main\t*\nfeature\t\n");
        assert_eq!(b.branches, vec!["main".to_string(), "feature".to_string()]);
        assert_eq!(b.current_branch.as_deref(), Some("main"));
    }

    #[test]
    fn build_push_args_no_options() {
        assert_eq!(build_push_args(None, None, None, false), vec!["push"]);
    }

    #[test]
    fn build_push_args_set_upstream_and_remote() {
        assert_eq!(
            build_push_args(Some("origin"), None, Some("main"), true),
            vec!["push", "--set-upstream", "origin", "main"],
        );
    }

    #[test]
    fn build_push_args_no_set_upstream_with_remote_option() {
        assert_eq!(
            build_push_args(None, Some("origin"), Some("main"), false),
            vec!["push", "origin", "main"],
        );
    }

    #[test]
    fn build_push_args_set_upstream_false_ignores_flag() {
        // set_upstream=false → no --set-upstream even with a remote
        assert_eq!(
            build_push_args(Some("upstream"), None, None, false),
            vec!["push", "upstream"],
        );
    }

    #[test]
    fn build_push_args_remote_name_overrides_remote() {
        // remote_name takes priority over remote
        assert_eq!(
            build_push_args(Some("upstream"), Some("origin"), None, true),
            vec!["push", "--set-upstream", "upstream"],
        );
    }

    #[test]
    fn build_auth_error_message_without_missing_password() {
        let msg = build_auth_error_message("clone", false);
        assert!(msg.contains("clone"), "action in message");
        assert!(msg.contains("requires credentials"), "correct variant");
    }

    #[test]
    fn build_auth_error_message_with_missing_password() {
        let msg = build_auth_error_message("push", true);
        assert!(msg.contains("push"), "action in message");
        assert!(msg.contains("requires a password/token"), "correct variant");
    }

    #[test]
    fn build_upstream_error_message_push_vs_pull() {
        let push_msg = build_upstream_error_message("push");
        assert!(push_msg.contains("push"), "push action in message");
        assert!(
            push_msg.contains("setUpstream"),
            "push hint mentions setUpstream"
        );

        let pull_msg = build_upstream_error_message("pull");
        assert!(pull_msg.contains("pull"), "pull action in message");
        assert!(
            pull_msg.contains("--set-upstream-to"),
            "pull hint mentions set-upstream-to"
        );
    }
}
