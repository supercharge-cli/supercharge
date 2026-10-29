use std::collections::HashMap;

use super::*;
use crate::items::{ImportPatternMode, ImportPermission, ImportRuleAction, ImportTool};
use crate::test_support::SOURCE;

fn snapshot(_: &Path) -> Option<AgentSettingsSnapshot> {
    Some(AgentSettingsSnapshot {
        permissions: vec![ImportPermission {
            action: ImportRuleAction::Allow,
            tool: ImportTool::Bash,
            pattern: Some("ls".into()),
            pattern_mode: ImportPatternMode::Glob,
        }],
        env: HashMap::from([
            ("B".to_string(), "2".to_string()),
            ("A".to_string(), "1".to_string()),
        ]),
    })
}

fn server(command: &str) -> McpServerConfig {
    toml::from_str(&format!("command = \"{command}\"")).unwrap()
}

fn inputs<'a>(
    cwd: &'a Path,
    home: &Path,
    settings_files: Vec<PathBuf>,
    load: &'a dyn Fn(&Path) -> Option<AgentSettingsSnapshot>,
) -> ScanInputs<'a> {
    ScanInputs {
        source: SOURCE,
        cwd,
        home: Some(home.to_path_buf()),
        settings_files,
        load_settings: load,
        user_mcp_servers: Vec::new(),
        project_mcp_servers: Vec::new(),
    }
}

#[test]
fn settings_split_by_scope_with_sorted_env() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let user_file = home.join(".otheragent").join("settings.json");
    let project_file = project.join(".otheragent").join("settings.json");
    let plan = scan_importable_settings(inputs(
        &project,
        &home,
        vec![user_file, project_file],
        &snapshot,
    ));
    assert_eq!(plan.global_items.len(), 3);
    assert_eq!(plan.project_items.len(), 3);
    let keys: Vec<&str> = plan
        .global_items
        .iter()
        .filter_map(|item| match item {
            ImportableItem::EnvVar { key, .. } => Some(key.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(keys, ["A", "B"]);
}

#[test]
fn unreadable_settings_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let none = |_: &Path| None;
    let plan = scan_importable_settings(inputs(
        dir.path(),
        dir.path(),
        vec![dir.path().join("missing.json")],
        &none,
    ));
    assert!(plan.is_empty());
}

#[test]
fn mcp_servers_land_in_their_scope() {
    let dir = tempfile::tempdir().unwrap();
    let none = |_: &Path| None;
    let mut scan = inputs(dir.path(), dir.path(), Vec::new(), &none);
    scan.user_mcp_servers = vec![("u".into(), server("u"))];
    scan.project_mcp_servers = vec![("p".into(), server("p"))];
    let plan = scan_importable_settings(scan);
    assert!(matches!(
        plan.global_items.as_slice(),
        [ImportableItem::McpServer { name, .. }] if name == "u"
    ));
    assert!(matches!(
        plan.project_items.as_slice(),
        [ImportableItem::McpServer { name, .. }] if name == "p"
    ));
}

#[test]
fn skill_and_rule_dirs_are_imported_per_scope() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("project");
    std::fs::create_dir_all(home.join(".otheragent").join("skills")).unwrap();
    std::fs::create_dir_all(project.join(".otheragent").join("rules")).unwrap();
    let none = |_: &Path| None;
    let plan = scan_importable_settings(inputs(&project, &home, Vec::new(), &none));
    assert!(matches!(
        plan.global_items.as_slice(),
        [ImportableItem::PathEntry {
            kind: PathKind::Skill,
            ..
        }]
    ));
    assert!(matches!(
        plan.project_items.as_slice(),
        [ImportableItem::PathEntry {
            kind: PathKind::Rule,
            ..
        }]
    ));
}

#[test]
fn project_dirs_that_are_the_user_dirs_are_not_imported_twice() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::create_dir_all(home.join(".otheragent").join("skills")).unwrap();
    let none = |_: &Path| None;
    // Running from the home directory: the project root is the home directory.
    let plan = scan_importable_settings(inputs(home, home, Vec::new(), &none));
    assert_eq!(plan.global_items.len(), 1);
    assert!(plan.project_items.is_empty(), "{:?}", plan.project_items);
}
