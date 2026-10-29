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

/// No clip transcriber ships yet; a static key keeps the streaming route.
#[tokio::test]
#[serial_test::serial]
async fn stt_routes_stream_with_a_static_key() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = Arc::new(AuthManager::new(dir.path(), GrokComConfig::default()));
    let routes = build_stt_routes(mgr.clone());
    assert!(routes.clip_transcriber.is_none());
    mgr.set_process_static_api_key(Some("stt-static-key".to_owned()));
    assert_eq!(Ok("stt-static-key".to_owned()), routes.auth.bearer().await);
}
