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
}
