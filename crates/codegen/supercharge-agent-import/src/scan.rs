use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tracing::{debug, info};

use supercharge_config_types::McpServerConfig;

use super::ImportSource;
use super::find_project_root;
use super::hooks::extract_hooks_from_settings_file;
use super::items::{AgentSettingsSnapshot, ImportPlan, ImportableItem, PathKind};

/// Everything the scan needs, already located and parsed by the caller.
pub struct ScanInputs<'a> {
    pub source: ImportSource<'a>,
    pub cwd: &'a Path,
    /// Home directory; settings files under `<home>/<config_dir_name>` are user-level.
    pub home: Option<PathBuf>,
    /// Settings JSON files in precedence order.
    pub settings_files: Vec<PathBuf>,
    /// Parses one settings file; `None` skips it.
    pub load_settings: &'a dyn Fn(&Path) -> Option<AgentSettingsSnapshot>,
    /// MCP servers configured for the user.
    pub user_mcp_servers: Vec<(String, McpServerConfig)>,
    /// MCP servers configured for the project.
    pub project_mcp_servers: Vec<(String, McpServerConfig)>,
}

/// Build the plan of everything importable, split into user and project scope.
#[must_use]
pub fn scan_importable_settings(inputs: ScanInputs<'_>) -> ImportPlan {
    let mut plan = ImportPlan::default();
    let user_dir = inputs
        .home
        .as_ref()
        .map(|home| home.join(inputs.source.config_dir_name));

    for path in &inputs.settings_files {
        let Some(settings) = (inputs.load_settings)(path) else {
            continue;
        };
        let is_global = user_dir.as_ref().is_some_and(|dir| path.starts_with(dir));
        let target = if is_global {
            &mut plan.global_items
        } else {
            &mut plan.project_items
        };
        target.extend(
            settings
                .permissions
                .into_iter()
                .map(ImportableItem::Permission),
        );
        let mut env: Vec<(String, String)> = settings.env.into_iter().collect();
        // HashMap order is random; keep the plan and the written config deterministic.
        env.sort();
        target.extend(
            env.into_iter()
                .map(|(key, value)| ImportableItem::EnvVar { key, value }),
        );
        target.extend(extract_hooks_from_settings_file(path));
    }

    plan.global_items
        .extend(inputs.user_mcp_servers.into_iter().map(|(name, config)| {
            ImportableItem::McpServer {
                name,
                config: Box::new(config),
            }
        }));
    plan.project_items.extend(
        inputs
            .project_mcp_servers
            .into_iter()
            .map(|(name, config)| ImportableItem::McpServer {
                name,
                config: Box::new(config),
            }),
    );

    scan_path_dirs(&inputs.source, user_dir.as_deref(), inputs.cwd, &mut plan);

    if !plan.is_empty() {
        info!(
            global = plan.global_items.len(),
            project = plan.project_items.len(),
            "Scanned settings from other coding agents for import"
        );
    }
    plan
}

/// Add `skills/` and `rules/` under the user and project config dirs as extra dirs.
/// A project dir that resolves to the user dir (cwd is the home directory) is skipped.
fn scan_path_dirs(
    source: &ImportSource<'_>,
    user_dir: Option<&Path>,
    cwd: &Path,
    plan: &mut ImportPlan,
) {
    const SUBDIRS: [(PathKind, &str); 2] = [(PathKind::Skill, "skills"), (PathKind::Rule, "rules")];
    let mut global_added: HashSet<PathBuf> = HashSet::new();

    if let Some(user_dir) = user_dir {
        for (kind, sub) in SUBDIRS {
            let dir = user_dir.join(sub);
            if dir.is_dir() {
                global_added.insert(dunce::canonicalize(&dir).unwrap_or_else(|_| dir.clone()));
                plan.global_items.push(ImportableItem::PathEntry {
                    kind,
                    path: dir.to_string_lossy().into_owned(),
                });
            }
        }
    }

    let project_dir = find_project_root(cwd).join(source.config_dir_name);
    for (kind, sub) in SUBDIRS {
        let dir = project_dir.join(sub);
        if !dir.is_dir() {
            continue;
        }
        let canonical = dunce::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        if global_added.contains(&canonical) {
            debug!(
                path = %dir.display(),
                "Skipping project dir that resolves to the same dir as the user entry"
            );
            continue;
        }
        plan.project_items.push(ImportableItem::PathEntry {
            kind,
            path: dir.to_string_lossy().into_owned(),
        });
    }
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
