//! Built-in REA mode: the bundled `reverse-engineer-anything` skill and its system-prompt block.
//!
//! The skill text is adapted from the MIT-licensed `morluto/rea` project; `assets/LICENSE` and
//! `assets/NOTICE.md` carry the license and attribution and are written out next to the skill.

use std::borrow::Cow;
use std::io;
use std::path::{Path, PathBuf};

/// Skill name, also the folder name under the built-in skills root.
pub const SKILL_NAME: &str = "reverse-engineer-anything";

/// Folder under the app home that holds skills shipped inside the binary.
pub const BUILTIN_SKILLS_DIR: &str = "builtin-skills";

/// Command users run themselves to register the REA MCP server. Never run automatically.
pub const SETUP_COMMAND: &str = "npx rea-agents setup";

const BLOCK_OPEN: &str = "<reverse-engineer-anything-mode>";
const BLOCK_CLOSE: &str = "</reverse-engineer-anything-mode>";

const SKILL_MD: &str = include_str!("assets/SKILL.md");

/// Every bundled file, as (path relative to the skill folder, contents).
pub const FILES: &[(&str, &str)] = &[
    ("SKILL.md", SKILL_MD),
    ("LICENSE", include_str!("assets/LICENSE")),
    ("NOTICE.md", include_str!("assets/NOTICE.md")),
    (
        "references/android-applications.md",
        include_str!("assets/references/android-applications.md"),
    ),
    (
        "references/connection-and-recovery.md",
        include_str!("assets/references/connection-and-recovery.md"),
    ),
    (
        "references/environments.md",
        include_str!("assets/references/environments.md"),
    ),
    (
        "references/evidence-workflows.md",
        include_str!("assets/references/evidence-workflows.md"),
    ),
    (
        "references/javascript-applications.md",
        include_str!("assets/references/javascript-applications.md"),
    ),
    (
        "references/native-and-artifacts.md",
        include_str!("assets/references/native-and-artifacts.md"),
    ),
    (
        "references/runtime-observation.md",
        include_str!("assets/references/runtime-observation.md"),
    ),
];

/// Root folder for built-in skills under `home`.
pub fn builtin_skills_root(home: &Path) -> PathBuf {
    home.join(BUILTIN_SKILLS_DIR)
}

/// Folder the REA skill is written to under `home`.
pub fn skill_dir(home: &Path) -> PathBuf {
    builtin_skills_root(home).join(SKILL_NAME)
}

/// Write the bundled skill under `home`, touching only files whose contents differ.
/// Returns the skill folder.
pub fn materialize(home: &Path) -> io::Result<PathBuf> {
    let dir = skill_dir(home);
    for (rel, contents) in FILES {
        let path = dir.join(rel);
        if std::fs::read(&path).is_ok_and(|existing| existing == contents.as_bytes()) {
            continue;
        }
        let parent = path.parent().unwrap_or(&dir);
        std::fs::create_dir_all(parent)?;
        let tmp = tempfile::NamedTempFile::new_in(parent)?;
        std::fs::write(tmp.path(), contents)?;
        tmp.persist(&path).map_err(|e| e.error)?;
    }
    Ok(dir)
}

/// The skill body without its YAML frontmatter.
fn skill_body() -> &'static str {
    let Some(rest) = SKILL_MD.strip_prefix("---") else {
        return SKILL_MD;
    };
    match rest.find("\n---") {
        Some(end) => rest[end + 4..].trim_start_matches(['\r', '\n']),
        None => SKILL_MD,
    }
}

/// System-prompt block injected while REA mode is on.
/// `skill_dir` is where the reference files were written, when that succeeded.
pub fn system_block(skill_dir: Option<&Path>) -> String {
    let references = match skill_dir {
        Some(dir) => format!(
            "The reference files linked below live in `{}`; read them from there when a route needs one.",
            dir.join("references").display()
        ),
        None => "The reference files are not available on disk; rely on this guide.".to_string(),
    };
    format!(
        "{BLOCK_OPEN}\n\
REA mode is on. Apply the reverse-engineer-anything guide below to reverse-engineering, binary, artifact and protocol analysis work.\n\
{references}\n\
If the `rea` MCP tools are not available, say so and suggest the user run `{SETUP_COMMAND}` themselves. Never run REA setup or install commands on your own.\n\
Guide adapted from morluto/rea (MIT License).\n\n\
{body}\n\
{BLOCK_CLOSE}",
        body = skill_body().trim_end(),
    )
}

/// Remove every REA block from `prompt`, including blocks written by older versions,
/// together with the blank line that separated it from the preceding text.
pub fn strip_system_block(prompt: &str) -> Cow<'_, str> {
    if !prompt.contains(BLOCK_OPEN) {
        return Cow::Borrowed(prompt);
    }
    let mut out = String::with_capacity(prompt.len());
    let mut rest = prompt;
    while let Some(start) = rest.find(BLOCK_OPEN) {
        let before = &rest[..start];
        out.push_str(before.strip_suffix("\n\n").unwrap_or(before));
        let block = &rest[start..];
        rest = match block.find(BLOCK_CLOSE) {
            Some(end) => &block[end + BLOCK_CLOSE.len()..],
            None => "",
        };
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// `prompt` with exactly one current REA block when `enabled`, and none otherwise.
/// Returns `prompt` unchanged when it already has the right shape, so the cached prefix survives.
pub fn apply_system_block<'a>(prompt: &'a str, block: &str, enabled: bool) -> Cow<'a, str> {
    if enabled && prompt.matches(BLOCK_OPEN).count() == 1 && prompt.contains(block) {
        return Cow::Borrowed(prompt);
    }
    let stripped = strip_system_block(prompt);
    if !enabled {
        return stripped;
    }
    Cow::Owned(format!("{stripped}\n\n{block}"))
}

/// Whether a `node` executable is on `PATH`. Looks at the file system only; spawns nothing.
pub fn node_on_path() -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let names: &[&str] = if cfg!(windows) {
        &["node.exe", "node.cmd"]
    } else {
        &["node"]
    };
    std::env::split_paths(&path).any(|dir| names.iter().any(|n| dir.join(n).is_file()))
}

/// One-line hint about registering the REA tools, adjusted for whether Node.js was found.
pub fn setup_hint(node_found: bool) -> String {
    if node_found {
        format!("For the REA tools, run `{SETUP_COMMAND}` yourself (it is never run for you).")
    } else {
        format!(
            "For the REA tools, install Node.js, then run `{SETUP_COMMAND}` yourself (it is never run for you)."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_skill_has_frontmatter_name_and_license() {
        assert!(SKILL_MD.starts_with("---"));
        assert!(SKILL_MD.contains(&format!("name: {SKILL_NAME}")));
        let license = FILES.iter().find(|(p, _)| *p == "LICENSE").unwrap().1;
        assert!(license.contains("MIT License"));
        assert!(license.contains("morluto"));
    }

    #[test]
    fn body_drops_frontmatter() {
        let body = skill_body();
        assert!(!body.starts_with("---"));
        assert!(!body.contains(&format!("name: {SKILL_NAME}\n")));
        assert!(!body.is_empty());
    }

    #[test]
    fn block_round_trips_through_strip() {
        let base = "You are helpful.";
        let dir = Path::new("/tmp/home/builtin-skills/reverse-engineer-anything");
        let block = system_block(Some(dir));
        let with = apply_system_block(base, &block, true).into_owned();
        assert_eq!(with, format!("{base}\n\n{block}"));
        assert!(with.contains("/tmp/home/builtin-skills/reverse-engineer-anything/references"));
        assert!(with.contains(SETUP_COMMAND));
        assert_eq!(strip_system_block(&with), base);
        assert!(matches!(strip_system_block(base), Cow::Borrowed(_)));
    }

    #[test]
    fn strip_removes_stale_and_unterminated_blocks() {
        let prompt = format!("a\n\n{BLOCK_OPEN}\nold text\n{BLOCK_CLOSE}\nb\n\n{BLOCK_OPEN}\ncut");
        assert_eq!(strip_system_block(&prompt), "a\nb");
    }

    #[test]
    fn apply_keeps_current_block_in_place_and_replaces_stale_ones() {
        let block = system_block(None);
        let current = format!("base\n\n{block}\n\n<other>tail</other>\n");
        assert!(matches!(
            apply_system_block(&current, &block, true),
            Cow::Borrowed(_)
        ));
        let stale = format!("base\n\n{BLOCK_OPEN}\nold\n{BLOCK_CLOSE}");
        assert_eq!(
            apply_system_block(&stale, &block, true),
            format!("base\n\n{block}")
        );
        assert_eq!(
            apply_system_block(&current, &block, false),
            "base\n\n<other>tail</other>\n"
        );
        assert!(matches!(
            apply_system_block("base", &block, false),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn materialize_writes_files_and_is_idempotent() {
        let home = tempfile::tempdir().unwrap();
        let dir = materialize(home.path()).unwrap();
        assert_eq!(dir, skill_dir(home.path()));
        for (rel, contents) in FILES {
            assert_eq!(std::fs::read_to_string(dir.join(rel)).unwrap(), *contents);
        }
        let skill = dir.join("SKILL.md");
        std::fs::write(&skill, "edited").unwrap();
        let notice_mtime = std::fs::metadata(dir.join("NOTICE.md"))
            .unwrap()
            .modified()
            .unwrap();
        materialize(home.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), SKILL_MD);
        assert_eq!(
            std::fs::metadata(dir.join("NOTICE.md"))
                .unwrap()
                .modified()
                .unwrap(),
            notice_mtime
        );
    }

    #[test]
    fn materialize_reports_unwritable_home() {
        let home = tempfile::tempdir().unwrap();
        let blocker = home.path().join("file");
        std::fs::write(&blocker, "x").unwrap();
        assert!(materialize(&blocker).is_err());
    }

    #[test]
    fn hint_mentions_node_only_when_missing() {
        assert!(setup_hint(false).contains("Node.js"));
        assert!(!setup_hint(true).contains("Node.js"));
        assert!(setup_hint(true).contains(SETUP_COMMAND));
    }
}
