use std::collections::HashSet;
use std::path::Path;

use toml::Value as TomlValue;
use toml::map::Map as TomlMap;
use tracing::{info, warn};

use supercharge_config_types::McpServerConfig;

use super::error::ImportError;
use super::hooks::apply_hooks_to_dir;
use super::items::{
    ImportPermission, ImportPlan, ImportResult, ImportRuleAction, ImportScope, ImportableItem,
    PathKind, format_rule_string,
};
use super::mcp::merge_mcp_servers;
use super::write::write_scoped;
use super::{CONFIG_FILE_NAME, ImportSource, PROJECT_CONFIG_DIR, find_project_root};

/// Write the plan into Supercharge config.
///
/// User items go to `<user_config_dir>/config.toml` and `<user_config_dir>/hooks/`;
/// project items go to `<repo>/.supercharge/`. Existing entries always win, and a
/// config file that is not valid TOML is left untouched and reported.
pub fn apply_import(
    source: &ImportSource<'_>,
    plan: &ImportPlan,
    cwd: &Path,
    user_config_dir: &Path,
) -> Result<ImportResult, ImportError> {
    let mut result = ImportResult::default();

    if !plan.global_items.is_empty() {
        result.global_count = apply_scope(
            source,
            &plan.global_items,
            user_config_dir,
            ImportScope::Global,
            &mut result.modified_files,
        )?;
    }
    if !plan.project_items.is_empty() {
        let project_dir = find_project_root(cwd).join(PROJECT_CONFIG_DIR);
        result.project_count = apply_scope(
            source,
            &plan.project_items,
            &project_dir,
            ImportScope::Project,
            &mut result.modified_files,
        )?;
    }
    Ok(result)
}

fn apply_scope(
    source: &ImportSource<'_>,
    items: &[ImportableItem],
    config_dir: &Path,
    scope: ImportScope,
    modified_files: &mut Vec<String>,
) -> Result<usize, ImportError> {
    let config_path = config_dir.join(CONFIG_FILE_NAME);
    let count = apply_items_to_config(&config_path, items, scope)?;
    if count > 0 {
        modified_files.push(config_path.to_string_lossy().into_owned());
    }
    let hooks_dir = config_dir.join("hooks");
    let hook_count = apply_hooks_to_dir(&hooks_dir, source.hooks_file_name, items, scope)?;
    if hook_count > 0 {
        modified_files.push(
            hooks_dir
                .join(source.hooks_file_name)
                .to_string_lossy()
                .into_owned(),
        );
    }
    Ok(count + hook_count)
}

/// Read a config file as a TOML table; a missing file is an empty table.
fn read_config(config_path: &Path, action: &'static str) -> Result<TomlValue, ImportError> {
    let root = match std::fs::read_to_string(config_path) {
        Ok(contents) => toml::from_str(&contents).map_err(|source| ImportError::InvalidToml {
            action,
            path: config_path.to_path_buf(),
            source,
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            TomlValue::Table(TomlMap::new())
        }
        Err(source) => return Err(ImportError::io("read", config_path, source)),
    };
    Ok(root)
}

fn write_config(
    config_path: &Path,
    root: &TomlValue,
    scope: ImportScope,
) -> Result<(), ImportError> {
    let toml_str = toml::to_string_pretty(root).map_err(|source| ImportError::TomlSerialize {
        path: config_path.to_path_buf(),
        source,
    })?;
    write_scoped(config_path, &toml_str, scope)
}

fn apply_items_to_config(
    config_path: &Path,
    items: &[ImportableItem],
    scope: ImportScope,
) -> Result<usize, ImportError> {
    let mut permissions: Vec<&ImportPermission> = Vec::new();
    let mut env_vars: Vec<(&str, &str)> = Vec::new();
    let mut mcp_servers: Vec<(&str, &McpServerConfig)> = Vec::new();
    let mut skill_dirs: Vec<&str> = Vec::new();
    let mut rule_dirs: Vec<&str> = Vec::new();
    for item in items {
        match item {
            ImportableItem::Permission(rule) => permissions.push(rule),
            ImportableItem::EnvVar { key, value } => env_vars.push((key, value)),
            ImportableItem::McpServer { name, config } => mcp_servers.push((name, config)),
            // Hooks go to the hooks JSON file, not config.toml.
            ImportableItem::Hook { .. } => {}
            ImportableItem::PathEntry { kind, path } => match kind {
                PathKind::Skill => skill_dirs.push(path),
                PathKind::Rule => rule_dirs.push(path),
            },
        }
    }
    if permissions.is_empty()
        && env_vars.is_empty()
        && mcp_servers.is_empty()
        && skill_dirs.is_empty()
        && rule_dirs.is_empty()
    {
        return Ok(0);
    }

    let mut root = read_config(config_path, "refusing to import")?;
    let table = root
        .as_table_mut()
        .ok_or_else(|| ImportError::not_a_table("config root"))?;

    let mut count = 0usize;
    if !permissions.is_empty() {
        count += merge_permissions(table, &permissions)?;
    }
    if !skill_dirs.is_empty() {
        count += merge_paths(table, "extra_skill_dirs", &skill_dirs)?;
    }
    if !rule_dirs.is_empty() {
        count += merge_paths(table, "extra_rule_dirs", &rule_dirs)?;
    }
    if !env_vars.is_empty() {
        count += merge_env_vars(table, &env_vars);
    }
    if !mcp_servers.is_empty() {
        count += merge_mcp_servers(table, &mcp_servers)?;
    }

    if count > 0 {
        write_config(config_path, &root, scope)?;
        info!(path = %config_path.display(), count, "Wrote imported settings to config.toml");
    }
    Ok(count)
}

/// Append strings missing from `table[section][key]`; returns how many were added.
fn append_missing_strings(
    table: &mut TomlMap<String, TomlValue>,
    section: &str,
    key: &str,
    new_values: impl IntoIterator<Item = String>,
) -> Result<usize, ImportError> {
    let section_table = table
        .entry(section)
        .or_insert_with(|| TomlValue::Table(TomlMap::new()))
        .as_table_mut()
        .ok_or_else(|| ImportError::not_a_table(format!("[{section}]")))?;
    let existing = section_table
        .entry(key)
        .or_insert_with(|| TomlValue::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| ImportError::NotAnArray {
            section: format!("{section}.{key}"),
        })?;
    let mut seen: HashSet<String> = existing
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    let mut count = 0usize;
    for value in new_values {
        if seen.insert(value.clone()) {
            existing.push(TomlValue::String(value));
            count += 1;
        }
    }
    Ok(count)
}

/// Merge rules into `[permission] allow|deny|ask` in compact form; duplicates are skipped.
fn merge_permissions(
    table: &mut TomlMap<String, TomlValue>,
    rules: &[&ImportPermission],
) -> Result<usize, ImportError> {
    let mut count = 0usize;
    for action in [
        ImportRuleAction::Allow,
        ImportRuleAction::Deny,
        ImportRuleAction::Ask,
    ] {
        let formatted: Vec<String> = rules
            .iter()
            .filter(|rule| rule.action == action)
            .map(|rule| format_rule_string(rule))
            .collect();
        if formatted.is_empty() {
            continue;
        }
        count += append_missing_strings(table, "permission", action.as_str(), formatted)?;
    }
    Ok(count)
}

/// Merge entries into `[paths] <key>`; duplicates are skipped.
fn merge_paths(
    table: &mut TomlMap<String, TomlValue>,
    key: &str,
    new_paths: &[&str],
) -> Result<usize, ImportError> {
    append_missing_strings(
        table,
        "paths",
        key,
        new_paths.iter().map(|p| (*p).to_string()),
    )
}

/// Merge variables into `[env]`; an existing key is never overwritten.
fn merge_env_vars(table: &mut TomlMap<String, TomlValue>, vars: &[(&str, &str)]) -> usize {
    let env = table
        .entry("env")
        .or_insert_with(|| TomlValue::Table(TomlMap::new()));
    let Some(env_table) = env.as_table_mut() else {
        warn!("[env] in config.toml is not a table, skipping env import");
        return 0;
    };
    let mut count = 0;
    for (key, value) in vars {
        if !env_table.contains_key(*key) {
            env_table.insert((*key).to_string(), TomlValue::String((*value).to_string()));
            count += 1;
        }
    }
    count
}

/// Whether the user config at `config_path` records a finished import for `source`.
/// A missing or unparseable file counts as not imported.
#[must_use]
pub fn is_import_marked_at(source: &ImportSource<'_>, config_path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(config_path) else {
        return false;
    };
    let Ok(value) = toml::from_str::<TomlValue>(&contents) else {
        return false;
    };
    value
        .get(source.marker_table)
        .and_then(|table| table.get("imported"))
        .and_then(TomlValue::as_bool)
        .unwrap_or(false)
}

/// Record a finished import as `[<marker_table>] imported = true` in the user config.
/// Other content is preserved; a config that is not valid TOML is left untouched.
pub fn write_import_marker(
    source: &ImportSource<'_>,
    config_path: &Path,
) -> Result<(), ImportError> {
    let mut root = read_config(config_path, "refusing to write import marker")?;
    let table = root
        .as_table_mut()
        .ok_or_else(|| ImportError::not_a_table("config root"))?;
    let marker = table
        .entry(source.marker_table)
        .or_insert_with(|| TomlValue::Table(TomlMap::new()))
        .as_table_mut()
        .ok_or_else(|| ImportError::not_a_table(format!("[{}]", source.marker_table)))?;
    marker.insert("imported".to_string(), TomlValue::Boolean(true));
    write_config(config_path, &root, ImportScope::Global)
}

#[cfg(test)]
#[path = "rewrite_tests.rs"]
mod tests;
