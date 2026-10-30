//! Shell side of REA mode: writes the built-in skill under the app home and reads the `[rea]` toggle.

use std::path::PathBuf;
use std::sync::OnceLock;

/// Write the built-in skills under the app home once per process.
/// Returns the REA skill folder, or `None` when it could not be written (logged, never fatal).
pub fn ensure_builtin_skills() -> Option<PathBuf> {
    static SKILL_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    SKILL_DIR
        .get_or_init(|| {
            let home = supercharge_tools::util::supercharge_home::supercharge_home();
            match supercharge_shared::rea::materialize(&home) {
                Ok(dir) => Some(dir),
                Err(error) => {
                    tracing::warn!(%error, "couldn't write the built-in REA skill");
                    None
                }
            }
        })
        .clone()
}

/// `[rea] enabled` from a loaded config tree; off when missing or not a bool.
pub fn enabled_in(config: &toml::Value) -> bool {
    config
        .get("rea")
        .and_then(|rea| rea.get("enabled"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// `[rea] enabled` from the effective config; off when the config can't be read.
pub fn enabled() -> bool {
    crate::util::config::load_effective_config()
        .ok()
        .is_some_and(|cfg| enabled_in(&cfg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertised_tool_names_match_registered_tools() {
        assert_eq!(
            supercharge_shared::rea::NATIVE_TOOLS,
            supercharge_tools::implementations::REA_TOOL_NAMES
        );
    }

    #[test]
    fn toggle_reads_only_a_bool_enabled_key() {
        let on: toml::Value = toml::from_str("[rea]\nenabled = true\n").unwrap();
        let off: toml::Value = toml::from_str("[rea]\nenabled = false\n").unwrap();
        let junk: toml::Value = toml::from_str("[rea]\nenabled = \"yes\"\n").unwrap();
        let none: toml::Value = toml::from_str("[ui]\nponytail = true\n").unwrap();
        assert!(enabled_in(&on));
        assert!(!enabled_in(&off));
        assert!(!enabled_in(&junk));
        assert!(!enabled_in(&none));
    }

    #[test]
    fn builtin_dir_name_matches_discovery() {
        assert_eq!(
            supercharge_shared::rea::BUILTIN_SKILLS_DIR,
            supercharge_agent::prompt::skills::BUILTIN_SKILLS_DIR
        );
    }
}
