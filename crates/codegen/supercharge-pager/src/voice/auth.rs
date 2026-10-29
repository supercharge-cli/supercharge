//! Bridge the shell's `AuthManager` onto the voice crate's STT routes.
//!
//! The streaming STT endpoint accepts both API keys and OAuth2 tokens and attributes per-user billing for OAuth.
//! The voice channel reuses the same bearer the agent uses for chat (no separate env var). No clip transcriber is
//! wired yet, so a login without a bearer reports [`VoiceAuthError::NotSignedIn`] and no socket opens.
//!
//! Resolved per request: the agent's refreshing manager in direct-spawn mode.
//! In leader mode, a non-refreshing one adopts the agent's rotated `auth.json` token under the file lock (see [`crate::acp`]).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use supercharge_tools::types::SharedApiKeyProvider;
use supercharge_voice::{SharedVoiceAuth, SttRoutes, VoiceAuthError, VoiceAuthProvider};

/// Adapts the shell's `ApiKeyProvider` onto [`VoiceAuthProvider`].
///
/// Resolves a token per request (never a static snapshot), so a long session follows the `AuthManager` instead of pinning a token that 401s.
struct AuthManagerVoiceAuth(SharedApiKeyProvider);

impl std::fmt::Debug for AuthManagerVoiceAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthManagerVoiceAuth")
    }
}

/// A blank key is as good as none: sending it would only earn a 401 from the socket.
fn bearer_result(key: Option<String>) -> Result<String, VoiceAuthError> {
    key.filter(|key| !key.trim().is_empty())
        .ok_or(VoiceAuthError::NotSignedIn)
}

impl VoiceAuthProvider for AuthManagerVoiceAuth {
    fn bearer(&self) -> Pin<Box<dyn Future<Output = Result<String, VoiceAuthError>> + Send + '_>> {
        let provider = self.0.clone();
        Box::pin(async move { bearer_result(provider.current_api_key_async().await) })
    }
}

/// Build the voice bearer provider from the connection's `AuthManager`.
///
/// Works for every auth method: session tokens and API keys / per-model keys.
fn build_voice_auth(auth_manager: Arc<supercharge_login::AuthManager>) -> SharedVoiceAuth {
    Arc::new(AuthManagerVoiceAuth(
        supercharge_login::shared_api_key_provider(auth_manager),
    ))
}

/// Streaming bearer, plus the clip transcriber on builds that can transcribe for the signed-in login. The pipeline
/// consults the transcriber only after [`VoiceAuthError::ForeignSession`], so an accepted bearer always streams.
pub fn build_stt_routes(auth_manager: Arc<supercharge_login::AuthManager>) -> SttRoutes {
    SttRoutes {
        auth: build_voice_auth(auth_manager),
        clip_transcriber: None,
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
