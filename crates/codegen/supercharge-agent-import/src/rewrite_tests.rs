use super::*;
use crate::test_support::SOURCE;

fn table_from(contents: &str) -> TomlMap<String, TomlValue> {
    toml::from_str(contents).unwrap()
}

fn read(path: &Path) -> TomlValue {
    toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn strings(value: Option<&TomlValue>) -> Vec<String> {
    value
        .and_then(TomlValue::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn allow(tool: crate::ImportTool, pattern: &str) -> ImportPermission {
    ImportPermission {
        action: ImportRuleAction::Allow,
        tool,
        pattern: Some(pattern.to_string()),
        pattern_mode: crate::ImportPatternMode::Glob,
    }
}

#[test]
fn merge_permissions_skips_existing_and_repeated_rules() {
    let mut table = table_from("[permission]\nallow = [\"Bash(npm test)\"]\n");
    let existing = allow(crate::ImportTool::Bash, "npm test");
    let new = allow(crate::ImportTool::Bash, "npm run build");
    let deny = ImportPermission {
        action: ImportRuleAction::Deny,
        ..allow(crate::ImportTool::Read, ".env")
    };
    let count = merge_permissions(&mut table, &[&existing, &new, &new, &deny]).unwrap();
    assert_eq!(count, 2);
    let permission = table.get("permission");
    assert_eq!(
        strings(permission.and_then(|p| p.get("allow"))),
        ["Bash(npm test)", "Bash(npm run build)"]
    );
    assert_eq!(
        strings(permission.and_then(|p| p.get("deny"))),
        ["Read(.env)"]
    );
    assert!(permission.and_then(|p| p.get("ask")).is_none());
}

#[test]
fn merge_permissions_reports_wrong_shapes() {
    let rule = allow(crate::ImportTool::Bash, "ls");
    let mut table = table_from("permission = 3\n");
    assert!(matches!(
        merge_permissions(&mut table, &[&rule]),
        Err(ImportError::NotATable { .. })
    ));
    let mut table = table_from("[permission]\nallow = \"Bash\"\n");
    assert!(matches!(
        merge_permissions(&mut table, &[&rule]),
        Err(ImportError::NotAnArray { .. })
    ));
}

#[test]
fn merge_env_vars_never_overwrites() {
    let mut table = table_from("[env]\nEXISTING = \"old\"\n");
    let count = merge_env_vars(&mut table, &[("EXISTING", "new"), ("NEW_VAR", "value")]);
    assert_eq!(count, 1);
    let env = table.get("env").unwrap();
    assert_eq!(env.get("EXISTING").and_then(TomlValue::as_str), Some("old"));
    assert_eq!(
        env.get("NEW_VAR").and_then(TomlValue::as_str),
        Some("value")
    );

    let mut table = TomlMap::new();
    assert_eq!(merge_env_vars(&mut table, &[("FOO", "bar")]), 1);

    let mut table = table_from("env = \"oops\"\n");
    assert_eq!(merge_env_vars(&mut table, &[("FOO", "bar")]), 0);
}

#[test]
fn merge_paths_creates_and_dedups() {
    let mut table = TomlMap::new();
    assert_eq!(
        merge_paths(&mut table, "extra_skill_dirs", &["/a", "/b", "/a"]).unwrap(),
        2
    );
    assert_eq!(
        merge_paths(&mut table, "extra_skill_dirs", &["/a", "/new"]).unwrap(),
        1
    );
    assert_eq!(
        strings(table.get("paths").and_then(|p| p.get("extra_skill_dirs"))),
        ["/a", "/b", "/new"]
    );
}

#[test]
fn marker_reads_only_an_explicit_true() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    assert!(!is_import_marked_at(&SOURCE, &path));
    for (contents, expected) in [
        ("[other]\nkey = \"value\"\n", false),
        ("[otheragent_compat]\nimported = false\n", false),
        ("[otheragent_compat]\nimported = \"yes\"\n", false),
        ("otheragent_compat = 1\n", false),
        ("not toml [", false),
        ("[otheragent_compat]\nimported = true\n", true),
    ] {
        std::fs::write(&path, contents).unwrap();
        assert_eq!(is_import_marked_at(&SOURCE, &path), expected, "{contents}");
    }
}

#[test]
fn marker_write_creates_preserves_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = dir.path().join("sub").join("config.toml");
    write_import_marker(&SOURCE, &fresh).unwrap();
    write_import_marker(&SOURCE, &fresh).unwrap();
    assert!(is_import_marked_at(&SOURCE, &fresh));

    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[other]\nkey = \"value\"\n\n[mcp_servers.foo]\ncommand = \"x\"\n",
    )
    .unwrap();
    write_import_marker(&SOURCE, &path).unwrap();
    let parsed = read(&path);
    assert_eq!(parsed["other"]["key"].as_str(), Some("value"));
    assert_eq!(parsed["mcp_servers"]["foo"]["command"].as_str(), Some("x"));
    assert!(is_import_marked_at(&SOURCE, &path));
}

#[test]
fn marker_write_refuses_invalid_toml_and_wrong_shape() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "broken = [\n").unwrap();
    let error = write_import_marker(&SOURCE, &path).unwrap_err();
    assert!(matches!(error, ImportError::InvalidToml { .. }), "{error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken = [\n");

    std::fs::write(&path, "otheragent_compat = 1\n").unwrap();
    assert!(matches!(
        write_import_marker(&SOURCE, &path),
        Err(ImportError::NotATable { .. })
    ));
}

#[test]
fn config_items_write_paths_and_skip_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let items = vec![
        ImportableItem::PathEntry {
            kind: PathKind::Skill,
            path: "/foo/skills".into(),
        },
        ImportableItem::PathEntry {
            kind: PathKind::Rule,
            path: "/bar/rules".into(),
        },
        ImportableItem::Hook {
            event: "Stop".into(),
            matcher: None,
            command: "true".into(),
            timeout: None,
        },
    ];
    assert_eq!(
        apply_items_to_config(&path, &items, ImportScope::Project).unwrap(),
        2
    );
    let parsed = read(&path);
    assert_eq!(
        strings(parsed["paths"].get("extra_skill_dirs")),
        ["/foo/skills"]
    );
    assert_eq!(
        strings(parsed["paths"].get("extra_rule_dirs")),
        ["/bar/rules"]
    );
    assert!(parsed.get("hooks").is_none());
}

#[test]
fn config_items_refuse_invalid_toml_without_touching_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[model\n").unwrap();
    let items = [ImportableItem::EnvVar {
        key: "A".into(),
        value: "b".into(),
    }];
    let error = apply_items_to_config(&path, &items, ImportScope::Project).unwrap_err();
    assert!(error.to_string().contains("not valid TOML"), "{error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[model\n");
}

#[test]
fn config_with_only_hooks_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[model\n").unwrap();
    let items = [ImportableItem::Hook {
        event: "Stop".into(),
        matcher: None,
        command: "true".into(),
        timeout: None,
    }];
    assert_eq!(
        apply_items_to_config(&path, &items, ImportScope::Project).unwrap(),
        0
    );
}

#[test]
fn apply_import_writes_both_scopes_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let user_dir = dir.path().join("user");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let mcp: McpServerConfig = toml::from_str("command = \"srv\"\n").unwrap();
    let plan = ImportPlan {
        global_items: vec![
            ImportableItem::Permission(allow(crate::ImportTool::Bash, "ls")),
            ImportableItem::McpServer {
                name: "docs".into(),
                config: Box::new(mcp),
            },
            ImportableItem::Hook {
                event: "Stop".into(),
                matcher: None,
                command: "true".into(),
                timeout: Some(3),
            },
        ],
        project_items: vec![ImportableItem::EnvVar {
            key: "A".into(),
            value: "b".into(),
        }],
    };

    let result = apply_import(&SOURCE, &plan, &project, &user_dir).unwrap();
    assert_eq!(result.global_count, 3);
    assert_eq!(result.project_count, 1);
    assert_eq!(result.modified_files.len(), 3);
    let user = read(&user_dir.join("config.toml"));
    assert_eq!(strings(user["permission"].get("allow")), ["Bash(ls)"]);
    assert_eq!(user["mcp_servers"]["docs"]["command"].as_str(), Some("srv"));
    assert!(
        user_dir
            .join("hooks")
            .join(SOURCE.hooks_file_name)
            .is_file()
    );
    let project_config = read(&project.join(".supercharge").join("config.toml"));
    assert_eq!(project_config["env"]["A"].as_str(), Some("b"));

    let again = apply_import(&SOURCE, &plan, &project, &user_dir).unwrap();
    assert_eq!(again.total(), 0);
    assert!(again.modified_files.is_empty());
}

#[cfg(unix)]
#[test]
fn apply_import_keeps_a_symlinked_user_config_linked() {
    let dir = tempfile::tempdir().unwrap();
    let user_dir = dir.path().join("user");
    std::fs::create_dir_all(&user_dir).unwrap();
    let real = dir.path().join("dotfiles.toml");
    std::fs::write(&real, "[ui]\ntheme = \"dark\"\n").unwrap();
    let link = user_dir.join("config.toml");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let plan = ImportPlan {
        global_items: vec![ImportableItem::EnvVar {
            key: "A".into(),
            value: "b".into(),
        }],
        project_items: Vec::new(),
    };

    apply_import(&SOURCE, &plan, dir.path(), &user_dir).unwrap();
    write_import_marker(&SOURCE, &link).unwrap();

    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let parsed = read(&real);
    assert_eq!(parsed["ui"]["theme"].as_str(), Some("dark"));
    assert_eq!(parsed["env"]["A"].as_str(), Some("b"));
    assert!(is_import_marked_at(&SOURCE, &link));
}
