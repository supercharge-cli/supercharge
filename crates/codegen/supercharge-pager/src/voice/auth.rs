//! Bridge the shell's `AuthManager` onto the voice crate's STT routes.
//!
//! The streaming STT endpoint accepts both API keys and OAuth2 tokens and attributes per-user billing for OAuth.
//! The voice channel reuses the same bearer the agent uses for chat (no separate env var). When the configured
//! provider is not the streaming host (or `[voice] model` / `base_url` is set), dictation records a clip and sends
//! it to that provider's standard `/audio/transcriptions` endpoint instead (see [`resolve_clip_route`]).
//!
//! Resolved per request: the agent's refreshing manager in direct-spawn mode.
//! In leader mode, a non-refreshing one adopts the agent's rotated `auth.json` token under the file lock (see [`crate::acp`]).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use supercharge_tools::types::SharedApiKeyProvider;
use supercharge_voice::{
    ClipRoute, HttpClipTranscriber, ProviderEndpoint, SharedVoiceAuth, SttRoutes, VoiceAuthError,
    VoiceAuthProvider, VoiceConfig, resolve_clip_route,
};

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

/// Always defers to the clip route: the pipeline only consults the transcriber after [`VoiceAuthError::ForeignSession`].
#[derive(Debug)]
struct ClipOnlyAuth;

impl VoiceAuthProvider for ClipOnlyAuth {
    fn bearer(&self) -> Pin<Box<dyn Future<Output = Result<String, VoiceAuthError>> + Send + '_>> {
        Box::pin(async { Err(VoiceAuthError::ForeignSession) })
    }
}

/// The clip route for this config and the agent's provider, or `None` to stream.
pub fn clip_route_for(voice_config: &VoiceConfig) -> Option<ClipRoute> {
    let provider = supercharge_shell::agent::provider_setup::load_existing_provider_setup_details()
        .map(|setup| ProviderEndpoint {
            base_url: setup.base_url,
            api_key: setup.api_key,
        });
    resolve_clip_route(
        &voice_config.transcription,
        provider.as_ref(),
        &voice_config.api_base,
        &|name| std::env::var(name).ok(),
    )
}

/// Streaming bearer, or (when `clip` is set) the clip transcriber for the provider's `/audio/transcriptions`.
pub fn build_stt_routes(
    auth_manager: Arc<supercharge_login::AuthManager>,
    clip: Option<ClipRoute>,
    user_agent: &str,
) -> SttRoutes {
    let agent_auth = build_voice_auth(auth_manager);
    let Some(clip) = clip else {
        return SttRoutes {
            auth: agent_auth,
            clip_transcriber: None,
        };
    };
    let mut transcriber =
        HttpClipTranscriber::new(clip.endpoint, user_agent).with_interims(clip.interim_transcripts);
    if clip.agent_key_allowed {
        transcriber = transcriber.with_fallback_key(agent_auth);
    }
    SttRoutes {
        auth: Arc::new(ClipOnlyAuth),
        clip_transcriber: Some(Arc::new(transcriber)),
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
