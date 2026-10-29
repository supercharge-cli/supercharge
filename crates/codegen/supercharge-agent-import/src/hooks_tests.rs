use super::*;

const FILE: &str = "imported-hooks.json";

fn write_settings(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, contents).unwrap();
    (dir, path)
}

fn hook(command: &str, timeout: Option<u64>) -> ImportableItem {
    ImportableItem::Hook {
        event: "PreToolUse".to_string(),
        matcher: Some("Bash".to_string()),
        command: command.to_string(),
        timeout,
    }
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn extract_basic_command() {
    let (_dir, path) = write_settings(
        r#"{"hooks": {"PreToolUse": [{"matcher": "Bash",
            "hooks": [{"type": "command", "command": "echo hi", "timeout": 7}]}]}}"#,
    );
    let items = extract_hooks_from_settings_file(&path);
    let [
        ImportableItem::Hook {
            event,
            matcher,
            command,
            timeout,
        },
    ] = items.as_slice()
    else {
        panic!("expected one hook: {items:?}");
    };
    assert_eq!(event, "PreToolUse");
    assert_eq!(matcher.as_deref(), Some("Bash"));
    assert_eq!(command, "echo hi");
    assert_eq!(*timeout, Some(7));
}

#[test]
fn extract_skips_non_command_handlers() {
    let (_dir, path) = write_settings(
        r#"{"hooks": {"PostToolUse": [{"hooks": [
            {"type": "http", "url": "https://example.com"},
            {"type": "command", "command": "true"}]}]}}"#,
    );
    assert_eq!(extract_hooks_from_settings_file(&path).len(), 1);
}

#[test]
fn extract_multiple_events_and_matchers() {
    let (_dir, path) = write_settings(
        r#"{"hooks": {
            "PreToolUse": [
                {"matcher": "Bash", "hooks": [{"type": "command", "command": "a"}]},
                {"matcher": "Edit", "hooks": [{"type": "command", "command": "b"}]}],
            "PostToolUse": [{"hooks": [
                {"type": "command", "command": "c"},
                {"type": "command", "command": "d"}]}]}}"#,
    );
    let items = extract_hooks_from_settings_file(&path);
    assert_eq!(items.len(), 4);
}

#[test]
fn extract_tolerates_bad_shapes() {
    for contents in [
        r#"{"permissions": {"allow": []}}"#,
        r#"{"hooks": "oops"}"#,
        r#"{"hooks": {"PreToolUse": "oops"}}"#,
        r#"{"hooks": {"PreToolUse": [{"hooks": "oops"}, 7, null]}}"#,
        r#"{"hooks": {"PreToolUse": [{"hooks": [{"type": "command"}, {"command": "x"}]}]}}"#,
        r#"{"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": 3}]}]}}"#,
        "not json",
        "[]",
    ] {
        let (_dir, path) = write_settings(contents);
        assert!(
            extract_hooks_from_settings_file(&path).is_empty(),
            "{contents}"
        );
    }
}

#[test]
fn extract_keeps_an_empty_command_and_drops_an_empty_matcher() {
    let (_dir, path) = write_settings(
        r#"{"hooks": {"PreToolUse": [{"matcher": "",
            "hooks": [{"type": "command", "command": "", "timeout": -1}]}]}}"#,
    );
    let items = extract_hooks_from_settings_file(&path);
    let [
        ImportableItem::Hook {
            matcher,
            command,
            timeout,
            ..
        },
    ] = items.as_slice()
    else {
        panic!("expected one hook: {items:?}");
    };
    assert_eq!(*matcher, None);
    assert_eq!(command, "");
    assert_eq!(*timeout, None, "a negative timeout is ignored");
}

#[test]
fn extract_missing_file_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(extract_hooks_from_settings_file(&dir.path().join("missing.json")).is_empty());
}

#[test]
fn apply_writes_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_dir = dir.path().join("hooks");
    let count = apply_hooks_to_dir(
        &hooks_dir,
        FILE,
        &[hook("echo x", None)],
        ImportScope::Project,
    )
    .unwrap();
    assert_eq!(count, 1);
    let parsed = read_json(&hooks_dir.join(FILE));
    assert_eq!(parsed["hooks"]["PreToolUse"][0]["matcher"], "Bash");
    assert_eq!(
        parsed["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "echo x"
    );
    assert!(
        parsed["hooks"]["PreToolUse"][0]["hooks"][0]
            .get("timeout")
            .is_none()
    );
}

#[test]
fn apply_dedups_existing_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_dir = dir.path().join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    std::fs::write(
        hooks_dir.join(FILE),
        r#"{"hooks": {"PreToolUse": [{"matcher": "Bash",
            "hooks": [{"type": "command", "command": "echo x"}]}]}}"#,
    )
    .unwrap();
    let items = [hook("echo x", None), hook("echo y", None)];
    let count = apply_hooks_to_dir(&hooks_dir, FILE, &items, ImportScope::Project).unwrap();
    assert_eq!(count, 1);
    let parsed = read_json(&hooks_dir.join(FILE));
    assert_eq!(parsed["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
}

#[test]
fn apply_refreshes_or_removes_timeout_on_dedup() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_dir = dir.path().join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let target = hooks_dir.join(FILE);
    std::fs::write(
        &target,
        r#"{"hooks": {"PreToolUse": [{"matcher": "Bash",
            "hooks": [{"type": "command", "command": "echo x", "timeout": 5}]}]}}"#,
    )
    .unwrap();

    let count = apply_hooks_to_dir(
        &hooks_dir,
        FILE,
        &[hook("echo x", Some(60))],
        ImportScope::Project,
    )
    .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        read_json(&target)["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"],
        60
    );

    apply_hooks_to_dir(
        &hooks_dir,
        FILE,
        &[hook("echo x", None)],
        ImportScope::Project,
    )
    .unwrap();
    let parsed = read_json(&target);
    assert_eq!(parsed["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
    assert!(
        parsed["hooks"]["PreToolUse"][0]["hooks"][0]
            .get("timeout")
            .is_none()
    );
}

#[test]
fn apply_does_not_rewrite_when_nothing_changed() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_dir = dir.path().join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let target = hooks_dir.join(FILE);
    let original = r#"{"hooks": {"PreToolUse": [{"matcher": "Bash",
        "hooks": [{"type": "command", "command": "echo x", "timeout": 5}]}]}}"#;
    std::fs::write(&target, original).unwrap();
    let count = apply_hooks_to_dir(
        &hooks_dir,
        FILE,
        &[hook("echo x", Some(5))],
        ImportScope::Project,
    )
    .unwrap();
    assert_eq!(count, 0);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), original);
}

#[test]
fn apply_replaces_a_malformed_file() {
    for original in ["not json", "[]", r#"{"hooks": 3}"#] {
        let dir = tempfile::tempdir().unwrap();
        let hooks_dir = dir.path().join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join(FILE), original).unwrap();
        let result = apply_hooks_to_dir(
            &hooks_dir,
            FILE,
            &[hook("echo x", None)],
            ImportScope::Project,
        );
        if original == r#"{"hooks": 3}"# {
            // A valid object with a wrong `hooks` type is reported, not clobbered.
            assert!(result.is_err());
            assert_eq!(
                std::fs::read_to_string(hooks_dir.join(FILE)).unwrap(),
                original
            );
        } else {
            assert_eq!(result.unwrap(), 1, "{original}");
        }
    }
}

#[test]
fn apply_with_no_hooks_is_a_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_dir = dir.path().join("hooks");
    let items = [ImportableItem::EnvVar {
        key: "X".into(),
        value: "y".into(),
    }];
    assert_eq!(
        apply_hooks_to_dir(&hooks_dir, FILE, &items, ImportScope::Project).unwrap(),
        0
    );
    assert!(!hooks_dir.exists());
}
