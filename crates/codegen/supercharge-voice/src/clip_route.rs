//! Decide whether dictation uses the streaming socket or the clip route to a standard
//! `/audio/transcriptions` endpoint, and with which base URL, key and model.
//!
//! `[voice] api_base` stays the streaming socket root. The clip route is configured with:
//! - `[voice] model`: transcription model id (default [`DEFAULT_TRANSCRIPTION_MODEL`]);
//! - `[voice] base_url`: standard API root (default: the configured chat provider);
//! - `[voice] api_key` / `[voice] env_key`: key or env var name (default: the provider's key);
//! - `[voice] interim_transcripts`: re-upload the growing clip for live previews (default false: each upload is billed).

use crate::http_transcriber::{DEFAULT_TRANSCRIPTION_MODEL, TranscriptionEndpoint};

/// Raw `[voice]` clip-route keys. Read key by key so one bad value never discards the rest.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TranscriptionSettings {
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub env_key: Option<String>,
    pub interim_transcripts: bool,
}

impl std::fmt::Debug for TranscriptionSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranscriptionSettings")
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<set>"))
            .field("env_key", &self.env_key)
            .field("interim_transcripts", &self.interim_transcripts)
            .finish()
    }
}

impl TranscriptionSettings {
    /// Non-empty string values of the clip-route keys in a `[voice]` table; other types are ignored.
    pub fn from_voice_table(voice: Option<&toml::Table>) -> Self {
        let get = |key: &str| {
            voice
                .and_then(|t| t.get(key))
                .and_then(toml::Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        Self {
            model: get("model"),
            base_url: get("base_url"),
            api_key: get("api_key"),
            env_key: get("env_key"),
            interim_transcripts: voice
                .and_then(|t| t.get("interim_transcripts"))
                .and_then(toml::Value::as_bool)
                .unwrap_or(false),
        }
    }

    /// The user asked for the clip route explicitly.
    pub fn is_explicit(&self) -> bool {
        self.model.is_some() || self.base_url.is_some()
    }
}

/// The chat provider the agent is configured with.
#[derive(Clone, Default)]
pub struct ProviderEndpoint {
    pub base_url: String,
    pub api_key: String,
}

/// A resolved clip route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipRoute {
    pub endpoint: TranscriptionEndpoint,
    /// When `endpoint.api_key` is empty, the agent's own key for this provider may be used:
    /// only true when the request goes to the provider's own host.
    pub agent_key_allowed: bool,
    pub interim_transcripts: bool,
}

/// Pick the clip route, or `None` to keep streaming.
///
/// Streaming stays the default when nothing is configured or the provider is the streaming host
/// itself; otherwise the clip route goes to `[voice] base_url` or the provider. Keys never cross
/// hosts: the provider's key (or the agent's) is only sent to the provider's own base URL.
pub fn resolve_clip_route(
    settings: &TranscriptionSettings,
    provider: Option<&ProviderEndpoint>,
    streaming_api_base: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<ClipRoute> {
    let provider = provider.filter(|p| !p.base_url.trim().is_empty());
    let provider_host = provider.and_then(|p| host_of(&p.base_url));
    let (base_url, same_as_provider) = match (&settings.base_url, provider) {
        (Some(base), _) => {
            let same = provider_host.is_some() && host_of(base) == provider_host;
            (base.clone(), same)
        }
        (None, Some(p)) => (p.base_url.trim().to_owned(), true),
        (None, None) => return None,
    };
    if !settings.is_explicit()
        && host_of(&base_url).is_some()
        && host_of(&base_url) == host_of(streaming_api_base)
    {
        return None;
    }
    let own_key = settings.api_key.clone().or_else(|| {
        settings
            .env_key
            .as_deref()
            .and_then(env)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    });
    let api_key = own_key
        .or_else(|| {
            same_as_provider
                .then(|| provider.map(|p| p.api_key.trim().to_owned()))
                .flatten()
                .filter(|k| !k.is_empty())
        })
        .unwrap_or_default();
    Some(ClipRoute {
        endpoint: TranscriptionEndpoint {
            base_url,
            api_key,
            model: settings
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_TRANSCRIPTION_MODEL.to_owned()),
        },
        agent_key_allowed: same_as_provider,
        interim_transcripts: settings.interim_transcripts,
    })
}

/// Lowercased host (with port) of a URL or bare host; `None` when unparseable.
fn host_of(base: &str) -> Option<String> {
    let base = base.trim();
    let parsed = url::Url::parse(base)
        .ok()
        .filter(|u| u.has_host())
        .or_else(|| url::Url::parse(&format!("https://{base}")).ok())?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    Some(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

#[cfg(test)]
#[path = "clip_route_tests.rs"]
mod tests;
