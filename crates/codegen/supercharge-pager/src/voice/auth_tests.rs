use super::{bearer_result, build_stt_routes};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use supercharge_login::{AuthManager, GrokComConfig};
use supercharge_voice::VoiceAuthError;

#[test]
fn missing_or_blank_key_is_not_signed_in() {
    assert_eq!(Err(VoiceAuthError::NotSignedIn), bearer_result(None));
    assert_eq!(
        Err(VoiceAuthError::NotSignedIn),
        bearer_result(Some("  ".to_owned()))
    );
    assert_eq!(Ok("k".to_owned()), bearer_result(Some("k".to_owned())));
}

/// Without a clip route a static key keeps the streaming route.
#[tokio::test]
#[serial_test::serial]
async fn stt_routes_stream_with_a_static_key() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = Arc::new(AuthManager::new(dir.path(), GrokComConfig::default()));
    let routes = build_stt_routes(mgr.clone(), None, "ua");
    assert!(routes.clip_transcriber.is_none());
    mgr.set_process_static_api_key(Some("stt-static-key".to_owned()));
    assert_eq!(Ok("stt-static-key".to_owned()), routes.auth.bearer().await);
}

fn clip(agent_key_allowed: bool) -> supercharge_voice::ClipRoute {
    supercharge_voice::ClipRoute {
        endpoint: supercharge_voice::TranscriptionEndpoint {
            base_url: "https://llm.example/v1".to_owned(),
            api_key: String::new(),
            model: "m".to_owned(),
        },
        agent_key_allowed,
        interim_transcripts: false,
    }
}

/// A clip route defers the bearer so the pipeline records a clip, even when a key is present.
#[tokio::test]
#[serial_test::serial]
async fn clip_route_defers_to_the_transcriber() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = Arc::new(AuthManager::new(dir.path(), GrokComConfig::default()));
    mgr.set_process_static_api_key(Some("stt-static-key".to_owned()));
    for allowed in [true, false] {
        let routes = build_stt_routes(mgr.clone(), Some(clip(allowed)), "ua");
        assert!(routes.clip_transcriber.is_some());
        assert_eq!(
            Err(VoiceAuthError::ForeignSession),
            routes.auth.bearer().await
        );
    }
}

/// Without any key and with the agent key disallowed, the transcriber fails with a clear message instead of panicking.
#[tokio::test]
#[serial_test::serial]
async fn clip_route_without_a_key_reports_how_to_fix_it() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = Arc::new(AuthManager::new(dir.path(), GrokComConfig::default()));
    let routes = build_stt_routes(mgr, Some(clip(false)), "ua");
    let error = routes
        .clip_transcriber
        .unwrap()
        .transcribe(supercharge_voice::AudioClip {
            wav: vec![0; 64],
            language: "en".to_owned(),
        })
        .await
        .unwrap_err();
    assert!(error.fatal);
    assert!(
        error.message.contains("[voice] api_key"),
        "{}",
        error.message
    );
}
