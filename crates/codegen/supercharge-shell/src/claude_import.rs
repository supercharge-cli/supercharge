//! Glue between the runtime compat layer and `supercharge_agent_import`.
//!
//! The import logic is source-agnostic and lives in `supercharge-agent-import`. This module
//! supplies the on-disk layout of the compat source, converts the workspace's parsed
//! permission rules, and owns the process-wide import marker used by runtime gates.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tracing::{debug, info};

use supercharge_agent_import::{
    AgentSettingsSnapshot, ImportPatternMode, ImportPermission, ImportRuleAction, ImportSource,
    ImportTool, ScanInputs,
};
use supercharge_workspace::permission::claude_settings::{
    find_claude_settings_paths, load_claude_settings,
};
use supercharge_workspace::permission::rules::parse_permission_rule;
use supercharge_workspace::permission::types::{
    PatternMode, PermissionRule, RuleAction, ToolFilter,
};

pub use supercharge_agent_import::{
    ImportPlan, ImportResult, ImportScope, ImportableItem, PathKind, find_project_root,
};

/// On-disk layout of the compat source. These names are existing user-facing paths and
/// config keys, so they must not change.
const SOURCE: ImportSource<'static> = ImportSource {
    config_dir_name: ".claude",
    hooks_file_name: "imported-from-claude.json",
    marker_table: "claude_compat",
};

fn user_config_path() -> PathBuf {
    crate::util::grok_home::grok_home().join(supercharge_agent_import::CONFIG_FILE_NAME)
}

/// Scan the compat source for everything importable into Supercharge config.
pub fn scan_importable_settings(cwd: &Path) -> ImportPlan {
    // The home used for the scope split must match config.rs and claude_import_state.rs,
    // or scan and hash tiers disagree.
    let load = |path: &Path| load_settings_snapshot(path);
    supercharge_agent_import::scan_importable_settings(ScanInputs {
        source: SOURCE,
        cwd,
        home: supercharge_dirs::home_dir(),
        settings_files: find_claude_settings_paths(cwd),
        load_settings: &load,
        // Known gap: this loader merges user-level and per-project entries, so per-project
        // servers are imported at user scope.
        user_mcp_servers: crate::util::config::load_claude_json_mcp_servers_as_configs_unfiltered(
            cwd,
        )
        .into_iter()
        .collect(),
        project_mcp_servers: crate::util::config::load_mcp_json_servers_as_configs_unfiltered(cwd)
            .into_iter()
            .collect(),
    })
}

fn load_settings_snapshot(path: &Path) -> Option<AgentSettingsSnapshot> {
    let settings = load_claude_settings(path)?;
    let mut permissions = Vec::new();
    if let Some(perms) = settings.permissions {
        for (action, entries) in [
            (RuleAction::Allow, perms.allow),
            (RuleAction::Deny, perms.deny),
            (RuleAction::Ask, perms.ask),
        ] {
            for rule_str in entries {
                match parse_permission_rule(&rule_str, action) {
                    Ok(rule) => match import_permission(&rule) {
                        Some(permission) => permissions.push(permission),
                        None => debug!(
                            path = %path.display(),
                            rule = %rule_str,
                            "Skipping permission rule for a tool the import does not know"
                        ),
                    },
                    Err(error) => debug!(
                        path = %path.display(),
                        rule = %rule_str,
                        error = %error,
                        "Skipping unparseable permission rule during import"
                    ),
                }
            }
        }
    }
    Some(AgentSettingsSnapshot {
        permissions,
        env: settings.env.unwrap_or_default().into_iter().collect(),
    })
}

/// `None` for a tool filter added to the workspace after this conversion was written.
fn import_permission(rule: &PermissionRule) -> Option<ImportPermission> {
    Some(ImportPermission {
        action: match rule.action {
            RuleAction::Allow => ImportRuleAction::Allow,
            RuleAction::Deny => ImportRuleAction::Deny,
            RuleAction::Ask => ImportRuleAction::Ask,
        },
        tool: match rule.tool {
            ToolFilter::Any => ImportTool::Any,
            ToolFilter::Bash => ImportTool::Bash,
            ToolFilter::Edit => ImportTool::Edit,
            ToolFilter::Read => ImportTool::Read,
            ToolFilter::Grep => ImportTool::Grep,
            ToolFilter::Mcp => ImportTool::Mcp,
            ToolFilter::WebFetch => ImportTool::WebFetch,
            ToolFilter::WebSearch => ImportTool::WebSearch,
            ToolFilter::AgentMessage => ImportTool::AgentMessage,
            _ => return None,
        },
        pattern: rule.pattern.clone(),
        pattern_mode: match rule.pattern_mode {
            PatternMode::Glob => ImportPatternMode::Glob,
            PatternMode::Domain => ImportPatternMode::Domain,
        },
    })
}

/// Apply a plan: user items to the Supercharge home, project items to `<repo>/.supercharge/`.
/// Additive only; existing entries always win. The caller writes the import marker.
pub fn apply_import(plan: &ImportPlan, cwd: &Path) -> anyhow::Result<ImportResult> {
    let user_dir = crate::util::grok_home::grok_home();
    Ok(supercharge_agent_import::apply_import(
        &SOURCE, plan, cwd, &user_dir,
    )?)
}

/// Cached result of [`is_claude_import_marked`]; see its doc for the caching rationale and trade-offs.
/// `RwLock<Option<bool>>` rather than `OnceLock<bool>` so tests can reset the state between cases.
/// The fast path is a read lock and a cached `bool`, far below the cost of the uncached `read_to_string` and TOML parse.
static MARKER_CACHE: std::sync::RwLock<Option<bool>> = std::sync::RwLock::new(None);

/// Whether the current user has already imported Claude settings. Reads `[claude_compat] imported = true` from `~/.supercharge/config.toml` once per process and caches the result.
/// When the marker is set, runtime fallbacks that read `.claude/` should be skipped; the user has migrated to native config. Resilient: returns `false` on missing file, missing section, parse error, or any other failure.
/// Trade-off: a user who manually flips the marker mid-session must restart to see the change, acceptable because reverting after import is rare. That variant logs one line so users can see the cutoff fired.
pub(crate) fn is_claude_import_marked() -> bool {
    if let Some(v) = *MARKER_CACHE.read().expect("MARKER_CACHE poisoned") {
        return v;
    }
    let v = is_claude_import_marked_at(&user_config_path());
    *MARKER_CACHE.write().expect("MARKER_CACHE poisoned") = Some(v);
    v
}

/// Forcibly seed the cache with the freshly written marker value.
/// The slash command calls this after `apply_import` writes the marker so gate checks reflect the new state without a restart.
pub(crate) fn refresh_marker_cache(value: bool) {
    *MARKER_CACHE.write().expect("MARKER_CACHE poisoned") = Some(value);
}

/// Reset the marker cache to uninitialised. Test-only.
#[cfg(test)]
pub(crate) fn reset_marker_cache_for_test() {
    *MARKER_CACHE.write().expect("MARKER_CACHE poisoned") = None;
}

/// Like [`is_claude_import_marked`], but logs a one-time `info!` line on the first true result per process.
/// `gate_name` identifies which call site fired the cutoff (useful for debugging which subsystem stopped reading `.claude/`).
/// Call sites are runtime fallback paths in `claude_compat.rs`, `util/config.rs`, `util/hooks.rs`, and `agent/config.rs` that previously read `.claude/`.
pub(crate) fn is_claude_import_marked_with_log(gate_name: &'static str) -> bool {
    static LOGGED: OnceLock<()> = OnceLock::new();
    let marked = is_claude_import_marked();
    if marked {
        LOGGED.get_or_init(|| {
            info!(
                first_gate = gate_name,
                "Claude compat disabled (marker set in config.toml)"
            );
        });
    }
    marked
}

/// Testable variant of [`is_claude_import_marked`] that reads from the given path.
pub(crate) fn is_claude_import_marked_at(config_path: &Path) -> bool {
    supercharge_agent_import::is_import_marked_at(&SOURCE, config_path)
}

/// Public entry point for the slash command: write the marker, log success, and seed the in-process cache so gate checks reflect it without restart.
/// Called from `/import-claude` even when nothing was imported: the marker is the user's opt-in choice, not a side effect of having imported items.
/// Re-entering a workspace with `.claude/` content must not re-engage the runtime fallbacks.
pub fn mark_claude_imported() -> anyhow::Result<()> {
    supercharge_agent_import::write_import_marker(&SOURCE, &user_config_path())?;
    refresh_marker_cache(true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use supercharge_workspace::HookSourceConfig;

    fn source_path_strs(sources: &[HookSourceConfig]) -> Vec<String> {
        sources
            .iter()
            .map(|s| match s {
                HookSourceConfig::SettingsFile(p) | HookSourceConfig::Directory(p) => {
                    p.to_string_lossy().into_owned()
                }
            })
            .collect()
    }

    #[test]
    fn workspace_rules_round_trip_through_the_import_format() {
        for (original, action) in [
            ("Bash(npm run build)", RuleAction::Allow),
            ("Read(src/*.rs)", RuleAction::Deny),
            ("WebFetch(domain:example.com)", RuleAction::Ask),
            ("Bash", RuleAction::Allow),
        ] {
            let parsed = parse_permission_rule(original, action).unwrap();
            let formatted =
                supercharge_agent_import::format_rule_string(&import_permission(&parsed).unwrap());
            assert_eq!(formatted, original);
            let reparsed = parse_permission_rule(&formatted, action).unwrap();
            assert_eq!(parsed.action, reparsed.action);
            assert_eq!(parsed.tool, reparsed.tool);
            assert_eq!(parsed.pattern, reparsed.pattern);
            assert_eq!(parsed.pattern_mode, reparsed.pattern_mode);
        }

        // The Bash `:*` prefix idiom formats to the bare prefix; reparsing stays equivalent.
        let parsed = parse_permission_rule("Bash(sed:*)", RuleAction::Deny).unwrap();
        assert_eq!(parsed.pattern.as_deref(), Some("sed"));
        let formatted =
            supercharge_agent_import::format_rule_string(&import_permission(&parsed).unwrap());
        let reparsed = parse_permission_rule(&formatted, RuleAction::Deny).unwrap();
        assert_eq!(parsed.tool, reparsed.tool);
        assert_eq!(parsed.pattern, reparsed.pattern);
        assert_eq!(parsed.pattern_mode, reparsed.pattern_mode);
    }

    #[test]
    fn settings_snapshot_keeps_parseable_rules_and_env() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"permissions": {"allow": ["Bash(ls)", "Bash("], "deny": ["Read(.env)"]},
                "env": {"A": "1"}}"#,
        )
        .unwrap();
        let snapshot = load_settings_snapshot(&path).unwrap();
        let formatted: Vec<String> = snapshot
            .permissions
            .iter()
            .map(supercharge_agent_import::format_rule_string)
            .collect();
        assert!(formatted.contains(&"Bash(ls)".to_string()), "{formatted:?}");
        assert!(
            formatted.contains(&"Read(.env)".to_string()),
            "{formatted:?}"
        );
        assert_eq!(snapshot.env.get("A").map(String::as_str), Some("1"));
        assert!(load_settings_snapshot(&dir.path().join("missing.json")).is_none());
    }

    #[test]
    fn is_claude_import_marked_at_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.toml");
        assert!(!is_claude_import_marked_at(&path));
    }

    #[test]
    fn is_claude_import_marked_at_missing_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\ntheme = \"dark\"\n").unwrap();
        std::fs::write(&path, "[other]\nkey = \"value\"\n").unwrap();
        assert!(!is_claude_import_marked_at(&path));
    }

    #[test]
    fn is_claude_import_marked_at_explicit_false() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[claude_compat]\nimported = false\n").unwrap();
        assert!(!is_claude_import_marked_at(&path));
    }

    #[test]
    fn is_claude_import_marked_at_true() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[claude_compat]\nimported = true\n").unwrap();
        assert!(is_claude_import_marked_at(&path));
    }

    // The MARKER_CACHE is a process-global RwLock so these tests must run serially
    // They each set the cache to true or false via the test helper, then call the gated function and assert on its early-return behavior
    use serial_test::serial;

    /// RAII guard that resets the marker cache when dropped, so tests don't leak state into one another.
    pub(super) struct MarkerGuard;
    impl Drop for MarkerGuard {
        fn drop(&mut self) {
            reset_marker_cache_for_test();
            // Also clear the workspace-side env-var override so it doesn't
            // leak into subsequent tests.
            unsafe { std::env::remove_var("_SUPERCHARGE_CLAUDE_MARKER_OVERRIDE") };
        }
    }

    /// RAII guard that removes an env var on drop, preventing leaks when
    /// an assertion panics before manual cleanup.

    #[test]
    #[serial]
    fn discover_hook_source_paths_skips_claude_when_marker_set() {
        let _g = MarkerGuard;
        refresh_marker_cache(true);
        let dir = tempfile::tempdir().unwrap();
        let compat = supercharge_tools::types::compat::CompatConfig::default();
        let paths = crate::util::hooks::discover_hook_source_paths(Some(dir.path()), &compat);
        let project_strs = source_path_strs(&paths.project);
        assert!(
            !project_strs.iter().any(|s| s.contains(".claude")),
            "project sources should not include .claude/ when marker set; got {:?}",
            project_strs
        );
        let global_strs = source_path_strs(&paths.global);
        assert!(
            !global_strs.iter().any(|s| s.contains("/.claude/")),
            "global sources should not include ~/.claude/ when marker set; got {:?}",
            global_strs
        );
    }

    #[test]
    #[serial]
    fn gate_load_claude_env_returns_empty_when_marker_set() {
        let _g = MarkerGuard;
        refresh_marker_cache(true);
        let dir = tempfile::tempdir().unwrap();
        let env = supercharge_workspace::permission::claude_settings::load_claude_env_with_project(
            dir.path(),
            true,
        );
        assert!(
            env.is_empty(),
            "load_claude_env_with_project should be empty when marker set"
        );
    }

    #[test]
    #[serial]
    fn discover_hook_source_paths_includes_claude_when_marker_unset() {
        let _g = MarkerGuard;
        refresh_marker_cache(false);
        let dir = tempfile::tempdir().unwrap();
        let compat = supercharge_tools::types::compat::CompatConfig::default();
        let paths = crate::util::hooks::discover_hook_source_paths(Some(dir.path()), &compat);
        let project_strs = source_path_strs(&paths.project);
        assert!(
            project_strs.iter().any(|s| s.contains(".claude")),
            "project sources should include .claude/ when marker unset; got {:?}",
            project_strs
        );
    }

    #[test]
    #[serial]
    fn discover_hook_source_paths_includes_cursor_hooks_json() {
        let _g = MarkerGuard;
        refresh_marker_cache(false);
        let dir = tempfile::tempdir().unwrap();
        let compat = supercharge_tools::types::compat::CompatConfig::default();
        let paths = crate::util::hooks::discover_hook_source_paths(Some(dir.path()), &compat);
        let global_strs = source_path_strs(&paths.global);
        assert!(
            global_strs
                .iter()
                .any(|s| s.contains(".cursor") && s.ends_with("hooks.json")),
            "global sources should include ~/.cursor/hooks.json; got {:?}",
            global_strs
        );
        let project_strs = source_path_strs(&paths.project);
        assert!(
            project_strs
                .iter()
                .any(|s| s.contains(".cursor") && s.ends_with("hooks.json")),
            "project sources should include .cursor/hooks.json; got {:?}",
            project_strs
        );
    }

    #[test]
    #[serial]
    fn discover_hook_source_paths_skips_cursor_when_disabled() {
        let _g = MarkerGuard;
        refresh_marker_cache(false);
        let dir = tempfile::tempdir().unwrap();
        let mut compat = supercharge_tools::types::compat::CompatConfig::default();
        compat.cursor.hooks = false;
        let paths = crate::util::hooks::discover_hook_source_paths(Some(dir.path()), &compat);
        let global_strs = source_path_strs(&paths.global);
        assert!(
            !global_strs.iter().any(|s| s.contains(".cursor")),
            "global sources should not include .cursor/ when disabled; got {:?}",
            global_strs
        );
        let project_strs = source_path_strs(&paths.project);
        assert!(
            !project_strs.iter().any(|s| s.contains(".cursor")),
            "project sources should not include .cursor/ when disabled; got {:?}",
            project_strs
        );
    }

    #[test]
    #[serial]
    fn discover_hook_source_paths_skips_claude_when_compat_disabled() {
        let _g = MarkerGuard;
        // Do NOT set the marker: test the compat gate in isolation
        refresh_marker_cache(false);
        let dir = tempfile::tempdir().unwrap();
        let mut compat = supercharge_tools::types::compat::CompatConfig::default();
        compat.claude.hooks = false;
        let paths = crate::util::hooks::discover_hook_source_paths(Some(dir.path()), &compat);
        let global_strs = source_path_strs(&paths.global);
        assert!(
            !global_strs.iter().any(|s| s.contains("/.claude/")),
            "global sources should not include ~/.claude/ when compat disabled; got {:?}",
            global_strs
        );
        let project_strs = source_path_strs(&paths.project);
        assert!(
            !project_strs.iter().any(|s| s.contains(".claude")),
            "project sources should not include .claude/ when compat disabled; got {:?}",
            project_strs
        );
    }

    #[test]
    #[serial]
    fn as_sources_gates_project_sources_on_trust() {
        // Trust gating lives in `HookSourcePaths::as_sources`: project sources are dropped when untrusted and kept when trusted
        // Assert on project sources (git_root-relative) since global sources use the real, non-injectable home
        let _g = MarkerGuard;
        refresh_marker_cache(false);
        let dir = tempfile::tempdir().unwrap();
        let compat = supercharge_tools::types::compat::CompatConfig::default();
        let paths = crate::util::hooks::discover_hook_source_paths(Some(dir.path()), &compat);
        assert!(
            !paths.project.is_empty(),
            "project source paths should be non-empty for a git_root"
        );

        let (global_untrusted, project) = paths.as_sources(false);
        assert_eq!(
            global_untrusted.len(),
            paths.global.len(),
            "global sources must survive untrusted"
        );
        assert!(
            project.is_empty(),
            "untrusted: as_sources(false) must drop all project sources"
        );

        let (_global, project) = paths.as_sources(true);
        assert!(
            !project.is_empty(),
            "trusted: as_sources(true) must keep project sources"
        );
    }

    #[test]
    #[serial]
    fn discover_hooks_honors_claude_compat_gate() {
        // Pins the single load entry point every startup/reload site uses
        // With `compat.claude.hooks = false` a project `.claude/settings.json` hook must NOT load; with it true it MUST
        // A pager e2e is disproportionate: the spawn/agent_ops wiring just forwards the resolved compat into this entry point
        let _g = MarkerGuard;
        // The marker stays unset so the import cutoff doesn't independently skip `.claude`; this isolates the compat gate
        refresh_marker_cache(false);

        // `discover_hooks` takes git_root directly (no git discovery), so a plain temp dir with a project `.claude/settings.json` suffices
        let git_root = tempfile::tempdir().unwrap();
        let claude_dir = git_root.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join("settings.json"),
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"claude_compat_gate_probe.sh"}]}]}}"#,
        )
        .unwrap();

        // Identify the probe by its unique raw command so real global hooks on the
        // test host (from the non-injectable ~/.claude, ~/.supercharge) don't interfere.
        let has_probe = |reg: &supercharge_hooks::discovery::HookRegistry| {
            reg.all_hooks().iter().any(|h| {
                h.command_raw
                    .as_deref()
                    .unwrap_or_default()
                    .contains("claude_compat_gate_probe")
            })
        };

        // Trusted so project sources are included; vary only the compat toggle.
        let mut compat = supercharge_tools::types::compat::CompatConfig::default();

        compat.claude.hooks = false;
        let (reg, _errs) = crate::util::hooks::discover_hooks(Some(git_root.path()), &compat, true);
        assert!(
            !has_probe(&reg),
            "compat.claude.hooks=false: project .claude hook must NOT be loaded"
        );

        compat.claude.hooks = true;
        let (reg, _errs) = crate::util::hooks::discover_hooks(Some(git_root.path()), &compat, true);
        assert!(
            has_probe(&reg),
            "compat.claude.hooks=true: project .claude hook must be loaded"
        );
    }

    #[test]
    #[serial]
    fn gate_load_mcp_json_servers_returns_empty_when_marker_set() {
        let _g = MarkerGuard;
        refresh_marker_cache(true);
        let dir = tempfile::tempdir().unwrap();
        let servers = crate::util::config::load_mcp_json_servers(dir.path());
        assert!(
            servers.is_empty(),
            "load_mcp_json_servers should be empty when marker set"
        );
    }

    #[test]
    #[serial]
    fn gate_load_claude_json_mcp_servers_returns_empty_when_marker_set() {
        let _g = MarkerGuard;
        refresh_marker_cache(true);
        let dir = tempfile::tempdir().unwrap();
        let compat = supercharge_tools::types::compat::CompatConfig::default();
        let servers = crate::util::config::load_claude_json_mcp_servers(dir.path(), &compat);
        assert!(
            servers.is_empty(),
            "load_claude_json_mcp_servers should be empty when marker set"
        );
    }

    #[tokio::test]
    #[serial]
    async fn gate_resolve_permissions_with_provenance_skips_claude_when_marker_set() {
        let _g = MarkerGuard;
        refresh_marker_cache(true);
        // Also set the env-var override so the workspace-resident marker
        // reader (which can't see the shell-side cache) honours the gate.
        unsafe { std::env::set_var("_SUPERCHARGE_CLAUDE_MARKER_OVERRIDE", "1") };
        let dir = tempfile::tempdir().unwrap();
        // Drop a Claude permissions file in the tempdir; with the marker set the gate should skip reading it
        let claude_dir = dir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join("settings.json"),
            r#"{ "permissions": { "allow": ["Bash(echo hi)"] } }"#,
        )
        .unwrap();

        // Note: `resolve_permissions_with_provenance` ALSO reads requirements, managed settings, and the developer's real `~/.supercharge/config.toml`.
        // We can't isolate `grok_home()` because it's `OnceLock`-cached.
        // Instead, assert on rule *provenance*: no rule should originate from our tempdir's `.claude/settings.json`. The dev's real ~/.supercharge config rules (if any) are out of scope for this test.
        let resolved =
            supercharge_workspace::permission::resolution::resolve_permissions_with_provenance(
                dir.path(),
                true,
            )
            .await
            .resolved;
        if let Some(r) = resolved {
            let tempdir_claude = claude_dir.join("settings.json");
            use supercharge_workspace::permission::types::RequirementSource;
            let leaked: Vec<&RequirementSource> = r
                .sources
                .iter()
                .filter(|s| matches!(s, RequirementSource::Settings { path } if path == &tempdir_claude))
                .collect();
            assert!(
                leaked.is_empty(),
                "tempdir .claude/settings.json should not produce rules when marker set, \
                 but got {} rule(s) from {}: {:?}",
                leaked.len(),
                tempdir_claude.display(),
                leaked
            );
        }
    }

    #[test]
    #[serial]
    fn gate_merge_claude_enabled_plugins_no_op_when_marker_set() {
        let _g = MarkerGuard;
        refresh_marker_cache(true);
        let mut plugins = crate::agent::config::PluginsConfig::default();
        let before_enabled = plugins.enabled.clone();
        let before_disabled = plugins.disabled.clone();
        // Pass `None` for cwd: the gate fires before any file IO
        plugins.merge_claude_enabled_plugins(None);
        assert_eq!(plugins.enabled, before_enabled);
        assert_eq!(plugins.disabled, before_disabled);
    }

    #[test]
    #[serial]
    fn gate_marker_cache_unset_means_uses_disk() {
        // Sanity test: with the cache reset, `is_claude_import_marked()` must (a) not panic and (b) populate the cache for subsequent reads
        // We intentionally **do not** assert a specific cached value: the dev's real `~/.supercharge/config.toml` may legitimately have the marker set during local testing, and we can't override `grok_home()`
        // It's `OnceLock`-cached, so any prior test that calls it locks the value in for the entire process The `MarkerGuard` resets the cache after this test, so subsequent gate tests start clean
        let _g = MarkerGuard;
        reset_marker_cache_for_test();
        let _ = is_claude_import_marked();
        assert!(
            MARKER_CACHE
                .read()
                .expect("MARKER_CACHE poisoned")
                .is_some(),
            "cache should be populated after a call"
        );
    }
}
