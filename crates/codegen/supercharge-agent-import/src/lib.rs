//! Import settings from other coding agents into Supercharge config.
//!
//! The crate is source-agnostic: callers describe the other agent's on-disk layout
//! with an [`ImportSource`] and pass in the settings they already parsed.
#![deny(clippy::indexing_slicing)]

mod error;
mod hooks;
mod items;
mod mcp;
mod rewrite;
mod scan;
mod summary;
mod write;

pub use error::ImportError;
pub use hooks::extract_hooks_from_settings_file;
pub use items::{
    AgentSettingsSnapshot, ImportPatternMode, ImportPermission, ImportPlan, ImportResult,
    ImportRuleAction, ImportScope, ImportTool, ImportableItem, PathKind, format_rule_string,
};
pub use rewrite::{apply_import, is_import_marked_at, write_import_marker};
pub use scan::{ScanInputs, scan_importable_settings};

use std::path::{Path, PathBuf};

/// Directory, relative to a project root, that holds project-level Supercharge config.
pub const PROJECT_CONFIG_DIR: &str = ".supercharge";
/// File name of a Supercharge config file inside a config directory.
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// On-disk layout of the coding agent being imported from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportSource<'a> {
    /// Name of the agent's config directory under the home directory and project root.
    /// Its `skills/` and `rules/` subdirectories are imported as extra directories.
    pub config_dir_name: &'a str,
    /// File name under `<supercharge config dir>/hooks/` that receives imported hooks.
    pub hooks_file_name: &'a str,
    /// Table in the user config whose `imported = true` key records a finished import.
    pub marker_table: &'a str,
}

/// Find the git repo root for project config writes; falls back to `cwd`.
#[must_use]
pub fn find_project_root(cwd: &Path) -> PathBuf {
    git2::Repository::discover(cwd)
        .ok()
        .and_then(|repo| repo.workdir().map(Path::to_path_buf))
        .unwrap_or_else(|| cwd.to_path_buf())
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::ImportSource;

    pub(crate) const SOURCE: ImportSource<'static> = ImportSource {
        config_dir_name: ".otheragent",
        hooks_file_name: "imported-hooks.json",
        marker_table: "otheragent_compat",
    };
}
