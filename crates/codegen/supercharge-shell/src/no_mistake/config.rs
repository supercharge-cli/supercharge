//! `[no_mistake]` configuration loaded from Supercharge config files.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// User and project settings for the validation gate.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct NoMistakeConfig {
    /// Integration branch used for rebase and for opening a pull request.
    pub base_branch: Option<String>,
    /// When true, a run publishes after the local gate unless `--no-publish` is set.
    pub publish: bool,
    pub commands: CommandConfig,
    pub auto_fix: AutoFixConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct CommandConfig {
    pub test: Option<String>,
    pub lint: Option<String>,
    pub document: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AutoFixConfig {
    pub review: u32,
    pub test: u32,
    pub document: u32,
    pub lint: u32,
}

impl Default for AutoFixConfig {
    fn default() -> Self {
        Self {
            review: 0,
            test: 3,
            document: 1,
            lint: 3,
        }
    }
}

impl NoMistakeConfig {
    /// Load `[no_mistake]` from the user config and an optional project file under `dir`.
    /// Missing or invalid files yield defaults rather than failing the run.
    pub fn load_from_disk(dir: &Path) -> Self {
        let mut cfg = Self::default();
        if let Some(user) = read_table(&supercharge_dirs::supercharge_home().join("config.toml")) {
            cfg.merge(user);
        }
        if let Some(project) = read_table(&dir.join(".supercharge").join("config.toml")) {
            cfg.merge(project);
        }
        cfg
    }

    pub fn from_toml_str(body: &str) -> Self {
        let mut cfg = Self::default();
        if let Some(parsed) = parse_table(body) {
            cfg.merge(parsed);
        }
        cfg
    }

    fn merge(&mut self, other: Self) {
        if other.base_branch.is_some() {
            self.base_branch = other.base_branch;
        }
        // `publish = true` in a later file must be able to turn it on;
        // a later explicit false also wins because we copy the field when
        // the table was present. Merge is last-wins for the whole struct
        // fields that were deserialized; missing keys stay default false
        // for publish, so only set when true or when the source set it.
        // We treat any provided table as authoritative for publish/commands
        // by replacing when the parsed struct is not a pure default besides
        // publish. Simpler: always copy publish from later files only if
        // the later file included a [no_mistake] table — `read_table`
        // returns None when the table is absent, so copying is correct.
        self.publish = other.publish;
        if other.commands.test.is_some() {
            self.commands.test = other.commands.test;
        }
        if other.commands.lint.is_some() {
            self.commands.lint = other.commands.lint;
        }
        if other.commands.document.is_some() {
            self.commands.document = other.commands.document;
        }
        self.auto_fix = other.auto_fix;
    }

    pub fn auto_fix_limit(&self, step: crate::no_mistake::report::StepName) -> u32 {
        use crate::no_mistake::report::StepName;
        match step {
            StepName::Review => self.auto_fix.review,
            StepName::Test => self.auto_fix.test,
            StepName::Document => self.auto_fix.document,
            StepName::Lint => self.auto_fix.lint,
            _ => 0,
        }
    }
}

#[derive(Deserialize)]
struct Root {
    no_mistake: Option<NoMistakeConfig>,
}

fn read_table(path: &PathBuf) -> Option<NoMistakeConfig> {
    let body = std::fs::read_to_string(path).ok()?;
    parse_table(&body)
}

fn parse_table(body: &str) -> Option<NoMistakeConfig> {
    let root: Root = toml::from_str(body).ok()?;
    root.no_mistake
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_table() {
        let cfg = NoMistakeConfig::from_toml_str(
            r#"
[no_mistake]
base_branch = "develop"
publish = true

[no_mistake.commands]
test = "cargo test -p demo"
lint = "cargo clippy"

[no_mistake.auto_fix]
review = 0
test = 2
lint = 1
"#,
        );
        assert_eq!(cfg.base_branch.as_deref(), Some("develop"));
        assert!(cfg.publish);
        assert_eq!(cfg.commands.test.as_deref(), Some("cargo test -p demo"));
        assert_eq!(cfg.auto_fix.test, 2);
        assert_eq!(cfg.auto_fix.review, 0);
    }

    #[test]
    fn missing_table_is_default() {
        let cfg = NoMistakeConfig::from_toml_str("[ui]\ntheme = \"dark\"\n");
        assert_eq!(cfg, NoMistakeConfig::default());
        assert_eq!(cfg.auto_fix.test, 3);
        assert!(!cfg.publish);
    }
}
