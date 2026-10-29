use super::*;

fn no_env(_: &str) -> Option<String> {
    None
}

fn provider() -> ProviderEndpoint {
    ProviderEndpoint {
        base_url: "https://llm.example/v1".to_owned(),
        api_key: "provider-key".to_owned(),
    }
}

const STREAMING: &str = "https://stream.example";

fn table(src: &str) -> toml::Table {
    src.parse().unwrap()
}

#[test]
fn nothing_configured_keeps_streaming() {
    let settings = TranscriptionSettings::default();
    assert_eq!(
        None,
        resolve_clip_route(&settings, None, STREAMING, &no_env)
    );
}

#[test]
fn provider_on_streaming_host_keeps_streaming() {
    let same = ProviderEndpoint {
        base_url: "https://stream.example/v1".to_owned(),
        api_key: "k".to_owned(),
    };
    let settings = TranscriptionSettings::default();
    assert_eq!(
        None,
        resolve_clip_route(&settings, Some(&same), STREAMING, &no_env)
    );
    // An explicit model still forces the clip route there.
    let settings = TranscriptionSettings {
        model: Some("m".to_owned()),
        ..Default::default()
    };
    let route = resolve_clip_route(&settings, Some(&same), STREAMING, &no_env).unwrap();
    assert_eq!("m", route.endpoint.model);
}

#[test]
fn other_provider_uses_clip_route_with_its_key_and_default_model() {
    let route = resolve_clip_route(
        &TranscriptionSettings::default(),
        Some(&provider()),
        STREAMING,
        &no_env,
    )
    .unwrap();
    assert_eq!("https://llm.example/v1", route.endpoint.base_url);
    assert_eq!("provider-key", route.endpoint.api_key);
    assert_eq!(DEFAULT_TRANSCRIPTION_MODEL, route.endpoint.model);
    assert!(route.agent_key_allowed);
}

#[test]
fn provider_key_never_goes_to_another_host() {
    let settings = TranscriptionSettings {
        base_url: Some("https://stt.example/v1".to_owned()),
        ..Default::default()
    };
    let route = resolve_clip_route(&settings, Some(&provider()), STREAMING, &no_env).unwrap();
    assert_eq!("https://stt.example/v1", route.endpoint.base_url);
    assert_eq!("", route.endpoint.api_key);
    assert!(!route.agent_key_allowed);
}

#[test]
fn same_host_override_keeps_the_provider_key() {
    let settings = TranscriptionSettings {
        base_url: Some("https://LLM.example/other".to_owned()),
        ..Default::default()
    };
    let route = resolve_clip_route(&settings, Some(&provider()), STREAMING, &no_env).unwrap();
    assert_eq!("provider-key", route.endpoint.api_key);
    assert!(route.agent_key_allowed);
}

#[test]
fn key_precedence_is_api_key_then_env_key_then_provider() {
    let env = |name: &str| (name == "STT_KEY").then(|| " env-key ".to_owned());
    let mut settings = TranscriptionSettings {
        env_key: Some("STT_KEY".to_owned()),
        ..Default::default()
    };
    let route = resolve_clip_route(&settings, Some(&provider()), STREAMING, &env).unwrap();
    assert_eq!("env-key", route.endpoint.api_key);
    settings.api_key = Some("own-key".to_owned());
    let route = resolve_clip_route(&settings, Some(&provider()), STREAMING, &env).unwrap();
    assert_eq!("own-key", route.endpoint.api_key);
    settings.api_key = None;
    settings.env_key = Some("MISSING".to_owned());
    let route = resolve_clip_route(&settings, Some(&provider()), STREAMING, &env).unwrap();
    assert_eq!("provider-key", route.endpoint.api_key);
}

#[test]
fn explicit_base_without_provider_works() {
    let settings = TranscriptionSettings {
        base_url: Some("http://127.0.0.1:9000/v1".to_owned()),
        model: Some("local-stt".to_owned()),
        ..Default::default()
    };
    let route = resolve_clip_route(&settings, None, STREAMING, &no_env).unwrap();
    assert_eq!("local-stt", route.endpoint.model);
    assert_eq!("", route.endpoint.api_key);
    assert!(!route.agent_key_allowed);
}

#[test]
fn model_only_without_any_base_keeps_streaming() {
    let settings = TranscriptionSettings {
        model: Some("m".to_owned()),
        ..Default::default()
    };
    assert_eq!(
        None,
        resolve_clip_route(&settings, None, STREAMING, &no_env)
    );
    let blank = ProviderEndpoint {
        base_url: "  ".to_owned(),
        api_key: "k".to_owned(),
    };
    assert_eq!(
        None,
        resolve_clip_route(&settings, Some(&blank), STREAMING, &no_env)
    );
}

#[test]
fn table_parsing_ignores_bad_types_and_blanks() {
    let voice = table(
        r#"
        model = 5
        base_url = "  https://stt.example/v1 "
        api_key = ""
        env_key = ["x"]
        interim_transcripts = "yes"
        "#,
    );
    let settings = TranscriptionSettings::from_voice_table(Some(&voice));
    assert_eq!(
        TranscriptionSettings {
            base_url: Some("https://stt.example/v1".to_owned()),
            ..Default::default()
        },
        settings
    );
    assert_eq!(
        TranscriptionSettings::default(),
        TranscriptionSettings::from_voice_table(None)
    );
}

#[test]
fn debug_hides_the_key() {
    let settings = TranscriptionSettings {
        api_key: Some("secret-value".to_owned()),
        ..Default::default()
    };
    assert!(!format!("{settings:?}").contains("secret-value"));
}

#[test]
fn host_parsing() {
    assert_eq!(
        Some("a.example".to_owned()),
        host_of("https://A.example/v1")
    );
    assert_eq!(Some("a.example".to_owned()), host_of("a.example"));
    assert_eq!(
        Some("a.example:8443".to_owned()),
        host_of("https://a.example:8443")
    );
    assert_eq!(None, host_of(""));
}

#[test]
fn interim_transcripts_flag_is_carried() {
    let voice = table("interim_transcripts = true\nbase_url = \"https://stt.example\"");
    let settings = TranscriptionSettings::from_voice_table(Some(&voice));
    assert!(settings.interim_transcripts);
    let route = resolve_clip_route(&settings, None, STREAMING, &no_env).unwrap();
    assert!(route.interim_transcripts);
}
