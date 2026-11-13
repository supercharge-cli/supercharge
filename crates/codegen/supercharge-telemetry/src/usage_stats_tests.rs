use super::*;

fn enabled_signals() -> Signals {
    Signals {
        key_present: true,
        ..Signals::default()
    }
}

#[test]
fn default_is_enabled_for_release_builds() {
    assert_eq!(decide(&enabled_signals()), Decision::Enabled);
}

#[test]
fn env_switch_off_disables() {
    for v in ["0", "false", "off", "no", "OFF", " 0 "] {
        let s = Signals {
            telemetry_env: Some(v.into()),
            ..enabled_signals()
        };
        assert_eq!(decide(&s), Decision::DisabledByEnv, "value {v:?}");
    }
}

#[test]
fn do_not_track_disables_even_when_forced_on() {
    for v in ["1", "true", "yes", "anything"] {
        let s = Signals {
            do_not_track_env: Some(v.into()),
            telemetry_env: Some("1".into()),
            ..enabled_signals()
        };
        assert_eq!(decide(&s), Decision::DisabledByDoNotTrack, "value {v:?}");
    }
    for v in ["", "0", "false"] {
        let s = Signals {
            do_not_track_env: Some(v.into()),
            ..enabled_signals()
        };
        assert_eq!(decide(&s), Decision::Enabled, "value {v:?}");
    }
}

#[test]
fn disable_telemetry_env_disables() {
    let s = Signals {
        disable_telemetry_env: Some("1".into()),
        ..enabled_signals()
    };
    assert_eq!(decide(&s), Decision::DisabledByEnv);
}

#[test]
fn command_and_config_opt_outs() {
    let s = Signals {
        command_setting: Some(false),
        ..enabled_signals()
    };
    assert_eq!(decide(&s), Decision::DisabledByCommand);
    let s = Signals {
        config_setting: Some(false),
        ..enabled_signals()
    };
    assert_eq!(decide(&s), Decision::DisabledByConfig);
    // Explicit env opt-in overrides a stored choice.
    let s = Signals {
        command_setting: Some(false),
        telemetry_env: Some("1".into()),
        ..enabled_signals()
    };
    assert_eq!(decide(&s), Decision::EnabledByEnv);
}

#[test]
fn ci_and_dev_builds_are_off_unless_forced() {
    let ci = Signals {
        ci: true,
        ..enabled_signals()
    };
    assert_eq!(decide(&ci), Decision::DisabledInCi);
    let dev = Signals {
        dev_build: true,
        ..enabled_signals()
    };
    assert_eq!(decide(&dev), Decision::DisabledDevBuild);
    let forced = Signals {
        ci: true,
        dev_build: true,
        telemetry_env: Some("on".into()),
        ..enabled_signals()
    };
    assert_eq!(decide(&forced), Decision::EnabledByEnv);
}

#[test]
fn no_key_is_off() {
    assert_eq!(decide(&Signals::default()), Decision::DisabledNoKey);
}

#[test]
fn test_process_never_sends() {
    // The test harness itself must resolve to "off" without any opt-out env.
    let signals = gather_signals(&PersistedState::default(), None);
    assert!(signals.ci, "test builds must count as CI");
    assert!(!decide(&signals).is_enabled() || signals.telemetry_env.is_some());
    assert!(
        !is_active(),
        "nothing initializes the recorder in unit tests"
    );
}

#[test]
fn compiled_key_is_a_public_ingestion_token() {
    assert!(compiled_project_key().starts_with("phc_"));
}

#[test]
fn state_round_trips_and_ids_are_uuids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(STATE_FILE_NAME);
    let mut state = load_state_from(&path);
    assert!(state.anonymous_id.is_none());
    let id = ensure_anonymous_id(&path, &mut state);
    assert!(uuid::Uuid::parse_str(&id).is_ok());
    let reloaded = load_state_from(&path);
    assert_eq!(reloaded.anonymous_id.as_deref(), Some(id.as_str()));
    // Stable across calls.
    let mut again = reloaded.clone();
    assert_eq!(ensure_anonymous_id(&path, &mut again), id);
    // A corrupted id is replaced, not sent.
    let mut bad = PersistedState {
        anonymous_id: Some("alice@example.com".into()),
        ..PersistedState::default()
    };
    let replaced = ensure_anonymous_id(&path, &mut bad);
    assert_ne!(replaced, "alice@example.com");
    assert!(uuid::Uuid::parse_str(&replaced).is_ok());
}

fn base() -> BaseContext {
    BaseContext {
        anonymous_id: "6f1f0b7e-1d0a-4c84-9a5e-6b8f1c2d3e4f".into(),
        app_version: "1.4.23".into(),
        os: os_label(),
        arch: std::env::consts::ARCH,
        install_method: Some("npm"),
        run_mode: RunMode::Tui,
        is_child: false,
    }
}

fn all_events() -> Vec<UsageEvent> {
    vec![
        UsageEvent::AppStarted,
        UsageEvent::CommandRun { command: "models" },
        UsageEvent::SessionStarted {
            kind: SessionKind::New,
        },
        UsageEvent::ProviderUsed {
            provider: "example-provider".into(),
            api_style: ApiStyle::ChatCompletions,
            model: "example-org/example-model-v1.5".into(),
        },
        UsageEvent::AcpSessionStarted {
            kind: SessionKind::Load,
        },
        UsageEvent::ErrorOccurred {
            category: ErrorCategory::Fatal,
            code: Some(1),
        },
    ]
}

/// The only property keys any event may carry.
const ALLOWED_KEYS: &[&str] = &[
    "distinct_id",
    "source",
    "app",
    "app_version",
    "platform",
    "$app_version",
    "$os",
    "os",
    "arch",
    "$process_person_profile",
    "$geoip_disable",
    "$ip",
    "run_mode",
    "child_process",
    "install_method",
    "$lib",
    "command",
    "session_kind",
    "provider",
    "api_style",
    "model",
    "kind",
    "code",
];

#[test]
fn payload_contains_only_allowlisted_fields() {
    for event in all_events() {
        let item = build_item(&base(), &event, "2026-11-14T00:00:00.000Z".into());
        let props = item
            .get("properties")
            .and_then(Value::as_object)
            .expect("properties object");
        for key in props.keys() {
            assert!(
                ALLOWED_KEYS.contains(&key.as_str()),
                "unexpected property {key} on {}",
                event.name()
            );
        }
        assert_eq!(props.get("$app_version"), Some(&json!("1.4.23")));
        assert_eq!(props.get("app_version"), Some(&json!("1.4.23")));
        assert_eq!(props.get("source"), Some(&json!("cli")));
        assert_eq!(props.get("$process_person_profile"), Some(&json!(false)));
        assert_eq!(props.get("$geoip_disable"), Some(&json!(true)));
        assert_eq!(props.get("$ip"), Some(&Value::Null));
        assert!(props.contains_key("$os"));
        assert_eq!(item.get("event"), Some(&json!(event.name())));
    }
}

#[test]
fn payload_never_contains_identity_or_secrets() {
    let user = std::env::var("USER").unwrap_or_default();
    let home = supercharge_dirs::home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let forbidden_keys = [
        "ip", "username", "user", "hostname", "host", "email", "path", "cwd", "prompt", "api_key",
        "token", "message", "content", "output",
    ];
    for event in all_events() {
        let item = build_item(&base(), &event, "2026-11-14T00:00:00.000Z".into());
        let props = item.get("properties").and_then(Value::as_object).unwrap();
        for key in forbidden_keys {
            assert!(
                !props.contains_key(key),
                "{key} present on {}",
                event.name()
            );
        }
        let text = item.to_string();
        for needle in [&home, &cwd] {
            if needle.len() > 3 {
                assert!(!text.contains(needle.as_str()), "leaked {needle}");
            }
        }
        if user.len() > 3 {
            assert!(!text.contains(&format!("\"{user}\"")), "leaked username");
        }
        assert!(
            item.get("api_key").is_none(),
            "token belongs on the batch body only"
        );
    }
}

#[test]
fn sanitizers_drop_paths_and_free_text() {
    assert_eq!(sanitize_model_id("/home/alice/models/llama.gguf"), "custom");
    assert_eq!(sanitize_model_id("C:\\Users\\bob\\m.gguf"), "custom");
    assert_eq!(sanitize_model_id("~/m"), "custom");
    assert_eq!(sanitize_model_id("../secret"), "custom");
    assert_eq!(sanitize_model_id("user@example.com"), "custom");
    assert_eq!(sanitize_model_id("sk-live key with spaces"), "custom");
    assert_eq!(sanitize_model_id("model-5.4"), "model-5.4");
    assert_eq!(
        sanitize_model_id("example-org/example-model-v4.1-flash"),
        "example-org/example-model-v4.1-flash"
    );
    assert_eq!(sanitize_model_id("coder2.5:7b"), "coder2.5:7b");
    assert_eq!(sanitize_label("example-provider"), "example-provider");
    assert_eq!(sanitize_label("agent stdio"), "agent stdio");
    assert_eq!(sanitize_label("https://corp.internal/v1"), "other");
    assert_eq!(sanitize_label("Some Text"), "other");
}

#[test]
fn batch_body_shape() {
    let item = build_item(&base(), &UsageEvent::AppStarted, "t".into());
    let body = batch_body("phc_test", vec![item]);
    assert_eq!(body.get("api_key"), Some(&json!("phc_test")));
    assert_eq!(body["batch"].as_array().map(Vec::len), Some(1));
}
