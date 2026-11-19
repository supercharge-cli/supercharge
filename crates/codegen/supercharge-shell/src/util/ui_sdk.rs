//! Shell side of the built-in Supercharge UI skill: writes it under the app home, detects
//! frontend workspaces, and builds the system-prompt blocks that switch it on.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use supercharge_shared::ui_sdk::ProjectFacts;
use supercharge_tools::implementations::ui_sdk::{FrontendProject, detect};

/// Write the bundled UI skill under the app home once per process.
/// Returns its folder, or `None` when it could not be written (logged, never fatal).
pub fn ensure_skill() -> Option<PathBuf> {
    static SKILL_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    SKILL_DIR
        .get_or_init(|| {
            let home = supercharge_tools::util::supercharge_home::supercharge_home();
            match supercharge_shared::ui_sdk::materialize(&home) {
                Ok(dir) => Some(dir),
                Err(error) => {
                    tracing::warn!(%error, "couldn't write the built-in UI skill");
                    None
                }
            }
        })
        .clone()
}

/// `[design] auto_skill` from a loaded config tree; on unless explicitly `false`.
pub fn auto_skill_in(config: &toml::Value) -> bool {
    config
        .get("design")
        .and_then(|design| design.get("auto_skill"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(true)
}

/// `[design] auto_skill` from the effective config; on when the config can't be read.
pub fn auto_skill_enabled() -> bool {
    crate::util::config::load_effective_config()
        .map(|cfg| auto_skill_in(&cfg))
        .unwrap_or(true)
}

/// How long one workspace's detection result is reused before looking at the disk again.
const DETECTION_TTL: Duration = Duration::from_secs(30);

/// The frontend project for `cwd`, cached briefly so per-turn checks stay cheap.
pub fn project_for(cwd: &Path) -> Option<FrontendProject> {
    type Cache = parking_lot::Mutex<Vec<(PathBuf, Instant, Option<FrontendProject>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    {
        let guard = cache.lock();
        if let Some((_, _, project)) = guard
            .iter()
            .find(|(dir, at, _)| dir == cwd && at.elapsed() < DETECTION_TTL)
        {
            return project.clone();
        }
    }
    let project = detect(cwd);
    let mut guard = cache.lock();
    guard.retain(|(dir, at, _)| dir != cwd && at.elapsed() < DETECTION_TTL);
    guard.push((cwd.to_path_buf(), Instant::now(), project.clone()));
    project
}

/// Prompt-ready facts about `project`, relative to the workspace `cwd`.
pub fn facts_for(project: &FrontendProject, cwd: &Path) -> ProjectFacts {
    ProjectFacts {
        frameworks: project.frameworks_label(),
        package_manager: project.package_manager.name().to_string(),
        install_command: project.install_command().display(),
        verify_commands: project
            .verify_commands()
            .iter()
            .map(|command| command.display())
            .collect(),
        relative_root: project.relative_root(cwd),
    }
}

/// The automatic UI skill block for `cwd`, when it is a frontend project and the skill is on.
pub fn auto_block(cwd: &Path, auto_skill: bool) -> Option<String> {
    if !auto_skill {
        return None;
    }
    let project = project_for(cwd)?;
    Some(supercharge_shared::ui_sdk::auto_block(
        &facts_for(&project, cwd),
        ensure_skill().as_deref(),
    ))
}

/// Environment switch for design mode in this process (`1`/`true`/`on` or `0`/`false`/`off`); `supercharge ui` sets it.
pub const DESIGN_MODE_ENV: &str = "SUPERCHARGE_DESIGN_MODE";

/// Usage line for `/design`.
pub const USAGE: &str = "Usage: /design on | off | status | <what to build>";

/// What `/design <args>` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesignOp {
    Status,
    On,
    Off,
    /// Build or change UI described in plain words, then verify it.
    Task(String),
}

impl DesignOp {
    pub fn parse(args: &str) -> Self {
        let args = args.trim();
        match args.to_ascii_lowercase().as_str() {
            "" | "status" => Self::Status,
            "on" | "enable" => Self::On,
            "off" | "disable" => Self::Off,
            _ => Self::Task(args.to_string()),
        }
    }
}

fn parse_switch(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

/// The design-mode override from [`DESIGN_MODE_ENV`], when it is set to a recognised value.
pub fn design_env_override() -> Option<bool> {
    std::env::var(DESIGN_MODE_ENV)
        .ok()
        .and_then(|value| parse_switch(&value))
}

/// `[design] enabled` from a loaded config tree; off unless explicitly `true`.
pub fn design_enabled_in(config: &toml::Value) -> bool {
    config
        .get("design")
        .and_then(|design| design.get("enabled"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// Whether design mode is on: the environment override wins over `[design] enabled`.
pub fn design_enabled() -> bool {
    design_env_override().unwrap_or_else(|| {
        crate::util::config::load_effective_config()
            .ok()
            .is_some_and(|cfg| design_enabled_in(&cfg))
    })
}

/// The design-mode system block for `cwd`.
pub fn design_block(cwd: &Path) -> String {
    let facts = project_for(cwd).map(|project| facts_for(&project, cwd));
    supercharge_shared::ui_sdk::design_block(facts.as_ref(), ensure_skill().as_deref())
}

/// The user message sent for `/design <task>` in `cwd`.
pub fn design_task_prompt(cwd: &Path, task: &str) -> String {
    let facts = project_for(cwd).map(|project| facts_for(&project, cwd));
    supercharge_shared::ui_sdk::design_task_prompt(task, facts.as_ref(), ensure_skill().as_deref())
}

fn project_line(cwd: &Path) -> String {
    match project_for(cwd) {
        Some(project) => {
            let facts = facts_for(&project, cwd);
            let root = facts
                .relative_root
                .map(|root| format!(" in {root}"))
                .unwrap_or_default();
            let verify = if facts.verify_commands.is_empty() {
                "no typecheck or build script".to_string()
            } else {
                facts.verify_commands.join(", then ")
            };
            let ui_sdk = if project.has_ui_sdk {
                "UI SDK installed"
            } else {
                "UI SDK not installed yet"
            };
            format!(
                "Project{root}: {}, {}, {ui_sdk}. Verify: {verify}.",
                facts.frameworks, facts.package_manager
            )
        }
        None => "No frontend project detected in this workspace.".to_string(),
    }
}

/// `/design status` text.
pub fn status_text(cwd: &Path, enabled: bool) -> String {
    let source = match design_env_override() {
        Some(_) => format!(" (set by {DESIGN_MODE_ENV} for this session)"),
        None => String::new(),
    };
    format!(
        "Design mode: {state}{source}.\n{project}\nUI skill: /{skill}, {auto}.\n{USAGE}",
        state = if enabled { "on" } else { "off" },
        project = project_line(cwd),
        skill = supercharge_shared::ui_sdk::SKILL_NAME,
        auto = if auto_skill_enabled() {
            "turns on by itself in frontend projects"
        } else {
            "automatic use is off ([design] auto_skill = false)"
        },
    )
}

/// Reply after `/design on` or `/design off` was saved.
pub fn toggled_text(cwd: &Path, on: bool) -> String {
    let mut text = if on {
        format!(
            "Design mode on. From the next message Supercharge works as a product designer: UI SDK components, accessible states, and a typecheck and build loop before it reports done.\n{}",
            project_line(cwd)
        )
    } else {
        "Design mode off.".to_string()
    };
    if let Some(forced) = design_env_override()
        && forced != on
    {
        text.push_str(&format!(
            "\n{DESIGN_MODE_ENV} keeps it {} for this session; the saved setting applies to new sessions.",
            if forced { "on" } else { "off" }
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_skill_is_on_unless_explicitly_off() {
        let on: toml::Value = toml::from_str("[design]\nauto_skill = true\n").unwrap();
        let off: toml::Value = toml::from_str("[design]\nauto_skill = false\n").unwrap();
        let junk: toml::Value = toml::from_str("[design]\nauto_skill = \"no\"\n").unwrap();
        let none: toml::Value = toml::from_str("[ui]\nponytail = true\n").unwrap();
        assert!(auto_skill_in(&on));
        assert!(!auto_skill_in(&off));
        assert!(auto_skill_in(&junk));
        assert!(auto_skill_in(&none));
    }

    #[test]
    fn auto_block_only_for_frontend_workspaces() {
        let web = tempfile::tempdir().unwrap();
        std::fs::write(
            web.path().join("package.json"),
            r#"{"dependencies":{"react-dom":"19"},"scripts":{"build":"vite build"}}"#,
        )
        .unwrap();
        std::fs::write(web.path().join("pnpm-lock.yaml"), "").unwrap();
        let block = auto_block(web.path(), true).expect("frontend workspace gets the block");
        assert!(block.contains("React, pnpm"));
        assert!(block.contains("pnpm add @ui-sdk/core @ui-sdk/react"));
        assert!(auto_block(web.path(), false).is_none(), "opt-out wins");

        let rust = tempfile::tempdir().unwrap();
        std::fs::write(rust.path().join("Cargo.toml"), "[package]\n").unwrap();
        assert!(auto_block(rust.path(), true).is_none());
    }

    #[test]
    fn facts_name_the_subproject_in_a_monorepo() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("apps").join("site");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("package.json"),
            r#"{"dependencies":{"svelte":"5"},"scripts":{"check":"svelte-check","build":"vite build"}}"#,
        )
        .unwrap();
        let project = project_for(root.path()).unwrap();
        let facts = facts_for(&project, root.path());
        assert_eq!(facts.relative_root.as_deref(), Some("apps/site"));
        assert_eq!(facts.verify_commands, ["npm run check", "npm run build"]);
    }

    #[test]
    fn builtin_dir_matches_discovery() {
        let home = Path::new("/h");
        assert_eq!(
            supercharge_shared::ui_sdk::skill_dir(home),
            home.join(supercharge_agent::prompt::skills::BUILTIN_SKILLS_DIR)
                .join(supercharge_shared::ui_sdk::SKILL_NAME)
        );
    }

    #[test]
    fn design_op_parses_toggles_and_tasks() {
        assert_eq!(DesignOp::parse(""), DesignOp::Status);
        assert_eq!(DesignOp::parse(" Status "), DesignOp::Status);
        assert_eq!(DesignOp::parse("ON"), DesignOp::On);
        assert_eq!(DesignOp::parse("disable"), DesignOp::Off);
        assert_eq!(
            DesignOp::parse("  a date picker with keyboard support "),
            DesignOp::Task("a date picker with keyboard support".into())
        );
    }

    #[test]
    fn design_toggle_reads_only_a_bool_enabled_key() {
        let on: toml::Value = toml::from_str("[design]\nenabled = true\n").unwrap();
        let off: toml::Value = toml::from_str("[design]\nenabled = false\n").unwrap();
        let junk: toml::Value = toml::from_str("[design]\nenabled = \"yes\"\n").unwrap();
        let none: toml::Value = toml::from_str("[rea]\nenabled = true\n").unwrap();
        assert!(design_enabled_in(&on));
        assert!(!design_enabled_in(&off));
        assert!(!design_enabled_in(&junk));
        assert!(!design_enabled_in(&none));
        assert_eq!(parse_switch("TRUE"), Some(true));
        assert_eq!(parse_switch(" off "), Some(false));
        assert_eq!(parse_switch("maybe"), None);
    }

    #[test]
    fn design_prompts_carry_the_project_and_the_verification_loop() {
        let web = tempfile::tempdir().unwrap();
        std::fs::write(
            web.path().join("package.json"),
            r#"{"dependencies":{"svelte":"5"},"scripts":{"check":"svelte-check","build":"vite build"}}"#,
        )
        .unwrap();
        std::fs::write(web.path().join("yarn.lock"), "").unwrap();
        let block = design_block(web.path());
        assert!(block.starts_with(supercharge_shared::ui_sdk::DESIGN_TAGS.open));
        assert!(block.contains("Svelte"), "{block}");
        assert!(block.contains("yarn run check"), "{block}");
        assert!(block.contains("ui_verify"));
        let task = design_task_prompt(web.path(), "  a sortable table ");
        assert!(
            task.starts_with("Design task: a sortable table\n"),
            "{task}"
        );
        let status = status_text(web.path(), false);
        assert!(status.contains("Design mode: off"), "{status}");
        assert!(status.contains("UI SDK not installed yet"), "{status}");

        let empty = tempfile::tempdir().unwrap();
        assert!(status_text(empty.path(), true).contains("No frontend project detected"));
        assert!(design_block(empty.path()).contains("No frontend project was detected"));
    }
}
