//! Interactive `/setup` provider configuration.
//!
//! Collects an OpenAI-compatible base URL and credential, lists models from
//! that endpoint, and writes the chosen model into `config.toml`.

use std::net::IpAddr;
use std::num::NonZeroU64;
use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;
use thiserror::Error;

use crate::agent::config::{self, ModelEntry, ModelEntryConfig};
use crate::remote::client::DEFAULT_CONTEXT_WINDOW;
use crate::sampling::ApiBackend;
use supercharge_sampler::AuthScheme;

const MAX_MODELS: usize = 500;
const MAX_BASE_URL_LEN: usize = 500;
const MAX_API_KEY_LEN: usize = 8192;
const MAX_MODEL_ID_LEN: usize = 200;

/// Reject values that are clearly UI/auth error text rather than credentials.
pub fn is_plausible_provider_api_key(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.len() > MAX_API_KEY_LEN {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    !lower.contains("authentication required")
        && !lower.contains("credentials were rejected")
        && !lower.contains("run /login")
        && !lower.contains("run `/login`")
        && !lower.contains("re-authenticate")
        && !lower.contains("session has expired")
}

fn sanitize_provider_api_key(raw: &str) -> String {
    if is_plausible_provider_api_key(raw) {
        raw.trim().to_owned()
    } else {
        String::new()
    }
}
pub const SETUP_PROVIDER_ID: &str = "setup";
pub const CUSTOM_PROVIDER_ID: &str = "custom";
pub const CUSTOM_MODEL_ID: &str = "__custom__";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SetupWireFormat {
    #[default]
    Openai,
    Anthropic,
    Responses,
}

impl SetupWireFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Responses => "responses",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Openai => "Chat Completions",
            Self::Anthropic => "Messages",
            Self::Responses => "Responses",
        }
    }

    pub fn api_backend(self) -> &'static str {
        match self {
            Self::Openai => "chat_completions",
            Self::Anthropic => "messages",
            Self::Responses => "responses",
        }
    }

    pub fn auth_scheme(self) -> &'static str {
        match self {
            Self::Openai | Self::Responses => "bearer",
            Self::Anthropic => "x_api_key",
        }
    }

    pub fn parse(raw: &str) -> Self {
        if raw.eq_ignore_ascii_case("anthropic") || raw.eq_ignore_ascii_case("messages") {
            Self::Anthropic
        } else if raw.eq_ignore_ascii_case("responses") {
            Self::Responses
        } else {
            Self::Openai
        }
    }

    pub fn from_api_backend(raw: &str) -> Self {
        Self::parse(raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetupProviderProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub base_url: &'static str,
    pub default_model: &'static str,
    pub format: SetupWireFormat,
    pub curated: &'static [&'static str],
}

/// Built-in provider profiles, plus a Custom row in the TUI.
pub fn setup_provider_profiles() -> &'static [SetupProviderProfile] {
    SETUP_PROVIDERS
}

pub fn find_setup_provider(id: &str) -> Option<&'static SetupProviderProfile> {
    SETUP_PROVIDERS.iter().find(|profile| profile.id == id)
}

const SETUP_PROVIDERS: &[SetupProviderProfile] = &[
    SetupProviderProfile {
        id: "pollinations",
        name: "Pollinations",
        description: "Recommended — large free/cheap model list",
        base_url: "https://gen.pollinations.ai/v1",
        default_model: "openai",
        format: SetupWireFormat::Openai,
        curated: &[
            "openai",
            "fast-model",
            "qwen-coder",
            "kimi-code",
            "deepseek",
        ],
    },
    SetupProviderProfile {
        id: "openai",
        name: "OpenAI",
        description: "api.openai.com",
        base_url: "https://api.openai.com/v1",
        default_model: "gpt-5.4",
        format: SetupWireFormat::Openai,
        curated: &["gpt-5.4", "gpt-4.1", "gpt-4o", "gpt-4o-mini"],
    },
    SetupProviderProfile {
        id: "anthropic",
        name: "Anthropic",
        description: "Claude via the Anthropic Messages API",
        base_url: "https://api.anthropic.com/v1",
        default_model: "claude-sonnet-4-6",
        format: SetupWireFormat::Anthropic,
        curated: &[
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "claude-haiku-4-5-20251001",
        ],
    },
    SetupProviderProfile {
        id: "openrouter",
        name: "OpenRouter",
        description: "Pay-per-use multi-provider aggregator",
        base_url: "https://openrouter.ai/api/v1",
        default_model: "anthropic/claude-sonnet-4-6",
        format: SetupWireFormat::Openai,
        curated: &[
            "anthropic/claude-sonnet-4-6",
            "openai/gpt-5.4",
            "google/gemini-2.5-pro",
            "deepseek/deepseek-chat-v3",
        ],
    },
    SetupProviderProfile {
        id: "deepseek",
        name: "DeepSeek",
        description: "V4 / chat / reasoner",
        base_url: "https://api.deepseek.com/v1",
        default_model: "deepseek-chat",
        format: SetupWireFormat::Openai,
        curated: &["deepseek-v4-pro", "deepseek-v4-flash", "deepseek-chat"],
    },
    SetupProviderProfile {
        id: "google",
        name: "Google AI Studio",
        description: "Gemini via OpenAI-compatible endpoint",
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        default_model: "gemini-2.5-pro",
        format: SetupWireFormat::Openai,
        curated: &["gemini-2.5-pro", "gemini-2.5-flash"],
    },
    SetupProviderProfile {
        id: "groq",
        name: "Groq",
        description: "Fast Llama inference",
        base_url: "https://api.groq.com/openai/v1",
        default_model: "llama-3.3-70b-versatile",
        format: SetupWireFormat::Openai,
        curated: &["llama-3.3-70b-versatile", "mixtral-8x7b-32768"],
    },
    SetupProviderProfile {
        id: "zai",
        name: "Z.AI / GLM",
        description: "Zhipu GLM direct API",
        base_url: "https://api.z.ai/api/paas/v4",
        default_model: "glm-5.1",
        format: SetupWireFormat::Openai,
        curated: &["glm-5.1", "glm-4.7"],
    },
    SetupProviderProfile {
        id: "kimi-coding",
        name: "Kimi Coding",
        description: "Moonshot OpenAI-compatible",
        base_url: "https://api.moonshot.ai/v1",
        default_model: "kimi-k2.6",
        format: SetupWireFormat::Openai,
        curated: &["kimi-k2.6", "kimi-k2.5"],
    },
    SetupProviderProfile {
        id: "kimi",
        name: "Moonshot (Kimi Anthropic)",
        description: "Kimi via Anthropic-compatible endpoint",
        base_url: "https://api.moonshot.ai/anthropic",
        default_model: "kimi-k2.6",
        format: SetupWireFormat::Anthropic,
        curated: &["kimi-k2.6", "kimi-k2.5"],
    },
    SetupProviderProfile {
        id: "qwen",
        name: "Qwen Cloud",
        description: "DashScope OpenAI-compatible",
        base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        default_model: "qwen3.6-plus",
        format: SetupWireFormat::Openai,
        curated: &["qwen3.6-plus", "qwen3-coder-plus", "qwen-max"],
    },
    SetupProviderProfile {
        id: "mistral",
        name: "Mistral AI",
        description: "Mistral / Codestral",
        base_url: "https://api.mistral.ai/v1",
        default_model: "large-model-latest",
        format: SetupWireFormat::Openai,
        curated: &["large-model-latest", "codestral-latest"],
    },
    SetupProviderProfile {
        id: "together",
        name: "Together AI",
        description: "Open models inference",
        base_url: "https://api.together.xyz/v1",
        default_model: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        format: SetupWireFormat::Openai,
        curated: &[
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
            "deepseek-ai/DeepSeek-V3",
        ],
    },
    SetupProviderProfile {
        id: "fireworks",
        name: "Fireworks AI",
        description: "OpenAI-compatible hosted models",
        base_url: "https://api.fireworks.ai/inference/v1",
        default_model: "accounts/fireworks/models/llama-v3p3-70b-instruct",
        format: SetupWireFormat::Openai,
        curated: &["accounts/fireworks/models/llama-v3p3-70b-instruct"],
    },
    SetupProviderProfile {
        id: "grok",
        name: "xAI (Grok)",
        description: "Grok direct API",
        base_url: "https://api.x.ai/v1",
        default_model: "grok-4-0709",
        format: SetupWireFormat::Openai,
        curated: &["grok-4-0709", "grok-3", "grok-3-mini"],
    },
    SetupProviderProfile {
        id: "routingrun",
        name: "Routing.run",
        description: "Open-source model routing",
        base_url: "https://api.routing.run/v1",
        default_model: "route/glm-5.1",
        format: SetupWireFormat::Openai,
        curated: &["route/glm-5.1", "deepseek-v4-pro"],
    },
    SetupProviderProfile {
        id: "zenllm",
        name: "ZenLLM",
        description: "200+ model aggregator",
        base_url: "https://api.zenllm.org/v1",
        default_model: "zhipu/glm-5.1",
        format: SetupWireFormat::Openai,
        curated: &["zhipu/glm-5.1", "deepseek/deepseek-v3"],
    },
    SetupProviderProfile {
        id: "nous",
        name: "Nous Research",
        description: "Nous Portal OAuth — free-tier models",
        base_url: "https://inference-api.nousresearch.com/v1",
        default_model: "Hermes-4.3-36B",
        format: SetupWireFormat::Openai,
        curated: &[
            "Hermes-4.3-36B",
            "Hermes-4-70B",
            "deepseek/deepseek-v4-flash",
        ],
    },
    SetupProviderProfile {
        id: "ollama",
        name: "Ollama (local)",
        description: "Local OpenAI-compatible server, key optional",
        base_url: "http://127.0.0.1:11434/v1",
        default_model: "llama3.3",
        format: SetupWireFormat::Openai,
        curated: &["llama3.3", "qwen2.5-coder", "codellama"],
    },
    SetupProviderProfile {
        id: "lmstudio",
        name: "LM Studio",
        description: "Local desktop server, key optional",
        base_url: "http://127.0.0.1:1234/v1",
        default_model: "local-model",
        format: SetupWireFormat::Openai,
        curated: &["local-model"],
    },
];

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderModel {
    pub id: String,
    pub name: String,
    /// True when this model accepts a reasoning / thinking effort level.
    pub supports_thinking: bool,
    /// Context window reported by the provider's `/models` entry, when present.
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSetup {
    pub base_url: String,
    pub api_key: String,
    pub model_id: String,
    pub model_name: String,
    pub format: SetupWireFormat,
    pub provider_id: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderSetupError {
    #[error("provider base URL is empty")]
    EmptyBaseUrl,
    #[error("provider base URL is too long")]
    BaseUrlTooLong,
    #[error("provider base URL must use http or https")]
    InvalidScheme,
    #[error("plain HTTP is only allowed for localhost development endpoints")]
    InsecureHttp,
    #[error("provider base URL is invalid")]
    InvalidUrl,
    #[error("provider API key looks like an error message, not a credential")]
    InvalidApiKey,
    #[error("provider API key is too long")]
    ApiKeyTooLong,
    #[error("model identifier is empty or too long")]
    InvalidModel,
    #[error("{0}")]
    Fetch(String),
    #[error("provider configuration I/O failed: {0}")]
    Io(String),
}

impl From<std::io::Error> for ProviderSetupError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

/// Trim, add https when missing, reject non-http(s), and drop a trailing `/models`.
pub fn normalize_provider_base_url(raw: &str) -> Result<String, ProviderSetupError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ProviderSetupError::EmptyBaseUrl);
    }
    if trimmed.len() > MAX_BASE_URL_LEN {
        return Err(ProviderSetupError::BaseUrlTooLong);
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = url::Url::parse(&with_scheme).map_err(|_| ProviderSetupError::InvalidUrl)?;
    match parsed.scheme() {
        "https" => {}
        "http" => {
            if !is_loopback_host(parsed.host_str().unwrap_or_default()) {
                return Err(ProviderSetupError::InsecureHttp);
            }
        }
        _ => return Err(ProviderSetupError::InvalidScheme),
    }
    if parsed.host_str().is_none() {
        return Err(ProviderSetupError::InvalidUrl);
    }
    let mut normalized = with_scheme.trim_end_matches('/').to_owned();
    if let Some(stripped) = normalized.strip_suffix("/models") {
        normalized = stripped.trim_end_matches('/').to_owned();
    }
    if normalized.ends_with("://") {
        return Err(ProviderSetupError::InvalidUrl);
    }
    Ok(normalized)
}

fn setup_wire_format_from_config(config: &toml::Value) -> SetupWireFormat {
    config
        .get("model_providers")
        .and_then(|providers| providers.get(SETUP_PROVIDER_ID))
        .and_then(|provider| provider.get("api_backend"))
        .and_then(toml::Value::as_str)
        .map(SetupWireFormat::from_api_backend)
        .unwrap_or(SetupWireFormat::Openai)
}

fn setup_profile_id_from_config(config: &toml::Value) -> Option<String> {
    config
        .get("model_providers")
        .and_then(|providers| providers.get(SETUP_PROVIDER_ID))
        .and_then(|provider| provider.get("id"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// Convert `/setup` model rows into catalog entries for `/model` and chat routing.
pub fn provider_models_to_catalog(
    models: Vec<ProviderModel>,
    base_url: &str,
    api_key: &str,
    format: SetupWireFormat,
) -> IndexMap<String, ModelEntry> {
    let api_backend = match format {
        SetupWireFormat::Openai => ApiBackend::ChatCompletions,
        SetupWireFormat::Anthropic => ApiBackend::Messages,
        SetupWireFormat::Responses => ApiBackend::Responses,
    };
    let auth_scheme = match format {
        SetupWireFormat::Openai | SetupWireFormat::Responses => AuthScheme::Bearer,
        SetupWireFormat::Anthropic => AuthScheme::XApiKey,
    };
    let mut extra_headers = IndexMap::new();
    if format == SetupWireFormat::Anthropic {
        extra_headers.insert("anthropic-version".to_owned(), "2023-06-01".to_owned());
    }
    let api_key = api_key.trim();
    let stored_key = if api_key.is_empty() {
        None
    } else {
        Some(api_key.to_owned())
    };
    let context_window =
        NonZeroU64::new(DEFAULT_CONTEXT_WINDOW).expect("default context window is non-zero");

    let mut catalog = IndexMap::with_capacity(models.len());
    for model in models {
        let entry_config = ModelEntryConfig {
            id: Some(model.id.clone()),
            model_family: None,
            model: model.id.clone(),
            base_url: base_url.to_owned(),
            name: Some(model.name.clone()),
            description: None,
            max_completion_tokens: None,
            temperature: None,
            top_p: None,
            api_key: stored_key.clone(),
            env_key: None,
            api_backend: api_backend.clone(),
            auth_scheme: Some(auth_scheme),
            reasoning_effort: None,
            // Thinking-capable provider models get the per-session `reasoning_effort`
            // select (off + the levels the model is known to accept).
            supports_reasoning_effort: model.supports_thinking,
            reasoning_efforts: if model.supports_thinking {
                let ids = reasoning_effort_ids_for_model(&model.id);
                let ids: &[&str] = if ids.is_empty() {
                    &["high", "medium", "low"]
                } else {
                    ids
                };
                crate::agent::session_config::effort_options_from_ids(ids)
            } else {
                Vec::new()
            },
            extra_headers: extra_headers.clone(),
            // The provider's reported window when known; the default only as a fallback.
            context_window: model
                .context_window
                .and_then(NonZeroU64::new)
                .unwrap_or(context_window),
            auto_compact_threshold_percent: None,
            system_prompt_label: None,
            api_base_url: None,
            use_concise: false,
            agent_type: config::default_agent_type(),
            inference_idle_timeout_secs: None,
            max_retries: None,
            hidden: false,
            supported_in_api: true,
            supports_backend_search: false,
            compactions_remaining: None,
            compaction_at_tokens: None,
            show_model_fingerprint: false,
            stream_tool_calls: None,
            laziness_detector: config::LazinessDetectorPerModelConfig::default(),
            ..Default::default()
        };
        catalog.insert(
            model.id.clone(),
            ModelEntry::from_config_entry(&entry_config),
        );
    }
    catalog
}

/// Fetch the provider catalog the same way `/setup` does and shape it for `/model`.
pub fn fetch_setup_provider_catalog(
    endpoints: &config::EndpointsConfig,
) -> Result<IndexMap<String, ModelEntry>, ProviderSetupError> {
    let toml_config = crate::config::load_effective_config()
        .map_err(|error| ProviderSetupError::Io(error.to_string()))?;
    let base_url = endpoints
        .models_base_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| endpoints.api_base_url.clone());
    let normalized = normalize_provider_base_url(base_url.as_str())?;
    let api_key = provider_setup_api_key_from_toml(&toml_config).unwrap_or_default();
    let format = setup_wire_format_from_config(&toml_config);
    let provider_id = setup_profile_id_from_config(&toml_config);
    let models = fetch_provider_models(&normalized, &api_key, format, provider_id.as_deref())?;
    Ok(provider_models_to_catalog(
        models,
        &normalized,
        &api_key,
        format,
    ))
}

pub fn provider_models_list_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.is_empty() {
        return String::new();
    }
    let lower = base.to_ascii_lowercase();
    if lower.ends_with("/v1")
        || lower.ends_with("/anthropic")
        || lower.ends_with("paas/v4")
        || lower.contains("/openai")
        || lower.contains("compatible-mode")
        || lower.contains("/v1/")
    {
        return format!("{base}/models");
    }
    format!("{base}/v1/models")
}

fn looks_anthropic(base_url: &str, format: SetupWireFormat) -> bool {
    if format == SetupWireFormat::Anthropic {
        return true;
    }
    let lower = base_url.to_ascii_lowercase();
    lower.contains("api.anthropic.com")
        || lower.ends_with("/anthropic")
        || lower.contains("/anthropic/")
}

fn is_non_chat_model(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    lower.contains("embed")
        || lower.contains("whisper")
        || lower.contains("tts")
        || lower.contains("dall-e")
        || lower.contains("dalle")
        || lower.contains("moderation")
        || lower.contains("image")
        || lower.contains("audio")
        || lower.contains("speech")
        || lower.contains("transcri")
}

/// Name-based guess for whether a model accepts a thinking/reasoning parameter.
/// Used only when the provider's `/models` metadata says nothing (see [`model_metadata_thinking`]).
pub fn infer_thinking_from_id(id: &str) -> bool {
    let s = id.to_ascii_lowercase();
    // Strip a vendor prefix such as `openai/` or `accounts/x/models/`.
    let s = s.rsplit('/').next().unwrap_or(&s);
    if s.contains("reason") || s.contains("think") {
        return true;
    }
    // Explicit non-thinking variants.
    if s.contains("non-reasoning") || s.contains("instruct") && !s.contains("qwen3") || s.contains("embed") {
        return false;
    }
    let tokens: Vec<&str> = s.split(|c: char| c == '-' || c == '_' || c == ':' || c == '.').collect();
    let first = tokens.first().copied().unwrap_or("");
    let version_after = |prefix: &str| -> Option<f32> {
        let rest = s.strip_prefix(prefix)?;
        let num: String = rest
            .trim_start_matches(['-', '_'])
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        num.trim_end_matches('.').parse().ok()
    };
    // o-series: o1, o3, o4-mini, ...
    if first.len() >= 2 && first.starts_with('o') && first[1..].chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    if let Some(v) = version_after("gpt") {
        return v >= 5.0 && !s.contains("chat-latest");
    }
    if s.starts_with("gpt-oss") {
        return true;
    }
    if s.starts_with("claude") {
        // claude-3-7-sonnet, claude-sonnet-4, claude-opus-4-1, claude-4.5-...
        let nums: Vec<u32> = tokens.iter().filter_map(|t| t.parse().ok()).collect();
        return match nums.as_slice() {
            [3, 7, ..] => true,
            [major, ..] => *major >= 4 && *major < 100,
            [] => false,
        };
    }
    if let Some(v) = version_after("gemini") {
        return v >= 2.5;
    }
    if s.starts_with("deepseek-r") || s.contains("deepseek-reasoner") || s.contains("-r1") {
        return true;
    }
    if s.starts_with("qwq") || s.starts_with("qwen3") || s.starts_with("qvq") {
        return true;
    }
    if s.starts_with("magistral") || s.starts_with("kimi-k2-thinking") || s.starts_with("glm-4.5") || s.starts_with("glm-4.6") || s.starts_with("glm-5") {
        return true;
    }
    false
}

/// Canonical effort ids a thinking model accepts, strongest first.
/// Recent frontier families (and ids that say `xhigh`/`-max`) get `xhigh`; other thinking
/// models get the three-level set. Non-thinking ids return an empty slice.
pub fn reasoning_effort_ids_for_model(id: &str) -> &'static [&'static str] {
    if !infer_thinking_from_id(id) {
        return &[];
    }
    let s = id.to_ascii_lowercase();
    let s = s.rsplit('/').next().unwrap_or(&s).to_owned();
    let gpt_ver: Option<f32> = s
        .strip_prefix("gpt-")
        .and_then(|r| {
            let n: String = r.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            n.trim_end_matches('.').parse().ok()
        });
    if gpt_ver.is_some_and(|v| v >= 5.1)
        || s.contains("xhigh")
        || s.contains("-max")
        || s.contains("4.6")
        || s.contains("4.20")
    {
        return &["xhigh", "high", "medium", "low"];
    }
    &["high", "medium", "low"]
}

/// What a `/models` entry says about thinking support, if anything.
/// Looks at explicit flags, `supported_parameters` (OpenRouter style), and nested
/// `capabilities`/`features`/`architecture` objects.
pub fn model_metadata_thinking(entry: &Value) -> Option<bool> {
    let obj = entry.as_object()?;
    const FLAGS: [&str; 8] = [
        "supports_reasoning_effort",
        "supportsReasoningEffort",
        "supports_reasoning",
        "supportsReasoning",
        "supports_thinking",
        "supportsThinking",
        "reasoning",
        "thinking",
    ];
    const PARAMS: [&str; 6] = [
        "reasoning",
        "reasoning_effort",
        "include_reasoning",
        "thinking",
        "enable_thinking",
        "thinking_budget",
    ];
    let flag_in = |o: &serde_json::Map<String, Value>| -> Option<bool> {
        FLAGS.iter().find_map(|k| match o.get(*k) {
            Some(Value::Bool(b)) => Some(*b),
            Some(Value::Object(inner)) => inner
                .get("supported")
                .or_else(|| inner.get("enabled"))
                .and_then(Value::as_bool)
                .or(Some(true)),
            _ => None,
        })
    };
    if let Some(b) = flag_in(obj) {
        return Some(b);
    }
    for key in ["reasoning_efforts", "reasoningEfforts"] {
        if let Some(a) = obj.get(key).and_then(Value::as_array) {
            return Some(!a.is_empty());
        }
    }
    for key in ["capabilities", "features", "architecture", "meta", "metadata"] {
        match obj.get(key) {
            Some(Value::Object(nested)) => {
                if let Some(b) = flag_in(nested) {
                    return Some(b);
                }
            }
            Some(Value::Array(items)) => {
                if items
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|i| matches!(i.to_ascii_lowercase().as_str(), "reasoning" | "thinking"))
                {
                    return Some(true);
                }
            }
            _ => {}
        }
    }
    for key in ["supported_parameters", "supportedParameters", "parameters"] {
        if let Some(params) = obj.get(key).and_then(Value::as_array) {
            if params.is_empty() {
                continue;
            }
            return Some(
                params
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|p| PARAMS.contains(&p.to_ascii_lowercase().as_str())),
            );
        }
    }
    None
}

/// Context window from a `/models` entry. Providers use different keys:
/// `context_window`, `context_length` (OpenRouter and most OpenAI-compatible gateways),
/// `max_context_length`, `max_input_tokens` (Anthropic-style), `contextWindow`,
/// `inputTokenLimit`, or nested `top_provider.context_length` / `limits.context_window`.
pub(crate) fn provider_model_context_window(entry: &Value) -> Option<u64> {
    let obj = entry.as_object()?;
    let as_u64 = |v: &Value| {
        v.as_u64()
            .or_else(|| v.as_f64().filter(|f| *f >= 1.0).map(|f| f as u64))
            .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
            .filter(|n| *n > 0)
    };
    const KEYS: [&str; 9] = [
        "context_window",
        "context_length",
        "max_context_length",
        "max_context_window",
        "contextWindow",
        "contextLength",
        "max_input_tokens",
        "maxInputTokens",
        "inputTokenLimit",
    ];
    KEYS.iter()
        .find_map(|k| obj.get(*k).and_then(as_u64))
        .or_else(|| {
            ["top_provider", "limits", "limit", "capabilities"]
                .iter()
                .filter_map(|k| obj.get(*k).and_then(Value::as_object))
                .find_map(|nested| {
                    KEYS.iter()
                        .chain(["context", "max_tokens"].iter())
                        .find_map(|k| nested.get(*k).and_then(as_u64))
                })
        })
}

fn model_entry_supports_thinking(entry: &Value, id: &str, name: &str) -> bool {
    model_metadata_thinking(entry)
        .unwrap_or_else(|| infer_thinking_from_id(id) || infer_thinking_from_id(name))
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']);
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// Parse an OpenAI- or Anthropic-compatible `/models` JSON body.
pub fn parse_openai_models_list(body: &Value) -> Vec<ProviderModel> {
    let entries = body
        .get("data")
        .or_else(|| body.get("models"))
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| body.as_array().cloned())
        .unwrap_or_default();

    let mut models = Vec::new();
    for entry in entries {
        let (id, name) = if let Some(id) = entry.as_str() {
            (id.trim(), id.trim())
        } else {
            let Some(obj) = entry.as_object() else {
                continue;
            };
            let Some(id) = obj
                .get("id")
                .or_else(|| obj.get("model"))
                .or_else(|| obj.get("modelId"))
                .or_else(|| obj.get("name"))
                .and_then(Value::as_str)
                .map(str::trim)
            else {
                continue;
            };
            let name = ["display_name", "title", "name"]
                .into_iter()
                .filter_map(|key| obj.get(key).and_then(Value::as_str))
                .map(str::trim)
                .find(|name| !name.is_empty())
                .unwrap_or(id);
            (id, name)
        };
        if id.is_empty() || id.len() > MAX_MODEL_ID_LEN || is_non_chat_model(id) {
            continue;
        }
        models.push(ProviderModel {
            id: id.to_owned(),
            name: name.to_owned(),
            context_window: provider_model_context_window(&entry),
            supports_thinking: model_entry_supports_thinking(&entry, id, name),
        });
        if models.len() >= MAX_MODELS {
            break;
        }
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);
    models
}

fn apply_catalog_auth(
    mut request: reqwest::blocking::RequestBuilder,
    base_url: &str,
    api_key: &str,
    format: SetupWireFormat,
) -> reqwest::blocking::RequestBuilder {
    let key = api_key.trim();
    if key.is_empty() {
        return request;
    }
    if looks_anthropic(base_url, format) {
        if key.starts_with("sk-ant-oat") || key.starts_with("eyJ") {
            request = request.bearer_auth(key);
        } else {
            request = request.header("x-api-key", key);
        }
        request = request.header("anthropic-version", "2023-06-01");
    } else {
        request = request.bearer_auth(key);
        if base_url.contains("openrouter.ai") {
            request = request.header("X-Title", "Supercharge AI");
        }
    }
    request
}

fn fetch_models_once(
    endpoint: &str,
    base_url: &str,
    api_key: &str,
    format: SetupWireFormat,
) -> Result<Vec<ProviderModel>, ProviderSetupError> {
    let client = crate::http::shared_startup_blocking_client();
    let request = apply_catalog_auth(client.get(endpoint), base_url, api_key, format);
    let response = request
        .send()
        .map_err(|error| ProviderSetupError::Fetch(error.to_string()))?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| ProviderSetupError::Fetch(error.to_string()))?;
    if !status.is_success() {
        let snippet = body.trim().chars().take(240).collect::<String>();
        return Err(ProviderSetupError::Fetch(if snippet.is_empty() {
            format!("provider returned HTTP {status}")
        } else {
            format!("provider returned HTTP {status}: {snippet}")
        }));
    }
    let parsed: Value = serde_json::from_str(&body)
        .map_err(|error| ProviderSetupError::Fetch(format!("invalid models JSON: {error}")))?;
    Ok(parse_openai_models_list(&parsed))
}

/// Fetch the live catalog using the saved credential, or anonymously when absent.
/// Failed requests and empty catalogs are errors, never curated-model fallbacks.
pub fn fetch_provider_models(
    base_url: &str,
    api_key: &str,
    format: SetupWireFormat,
    provider_id: Option<&str>,
) -> Result<Vec<ProviderModel>, ProviderSetupError> {
    let mut api_key = api_key.to_owned();
    if provider_id == Some("nous") {
        match super::nous_oauth::resolve_access_token() {
            Ok(token) => api_key = token,
            Err(err) if api_key.trim().is_empty() => {
                return Err(ProviderSetupError::Fetch(err));
            }
            Err(_) => {}
        }
    }
    let api_key = api_key.as_str();
    let base_url = normalize_provider_base_url(base_url)?;
    if api_key.len() > MAX_API_KEY_LEN {
        return Err(ProviderSetupError::ApiKeyTooLong);
    }
    let endpoint = provider_models_list_url(&base_url);
    let models = fetch_models_once(&endpoint, &base_url, api_key, format)?;
    if models.is_empty() {
        return Err(ProviderSetupError::Fetch(
            "provider returned no models".to_owned(),
        ));
    }
    Ok(models)
}

pub fn persist_provider_setup(setup: &ProviderSetup) -> Result<(), ProviderSetupError> {
    persist_provider_setup_at(&crate::util::config::user_config_path(), setup)
}

pub fn persist_provider_setup_at(
    path: &Path,
    setup: &ProviderSetup,
) -> Result<(), ProviderSetupError> {
    let base_url = normalize_provider_base_url(&setup.base_url)?;
    let api_key = sanitize_provider_api_key(&setup.api_key);
    if !setup.api_key.trim().is_empty() && api_key.is_empty() {
        return Err(ProviderSetupError::InvalidApiKey);
    }
    if api_key.len() > MAX_API_KEY_LEN {
        return Err(ProviderSetupError::ApiKeyTooLong);
    }
    let model_id = setup.model_id.trim();
    if model_id.is_empty() || model_id.len() > MAX_MODEL_ID_LEN {
        return Err(ProviderSetupError::InvalidModel);
    }
    let model_name = setup.model_name.trim();
    let model_name = if model_name.is_empty() {
        model_id
    } else {
        model_name
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut document = match crate::util::config::read_to_string_or_empty(path) {
        Ok(content) if !content.trim().is_empty() => content
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| ProviderSetupError::Io(error.to_string()))?,
        Ok(_) => toml_edit::DocumentMut::new(),
        Err(error) => return Err(error.into()),
    };

    apply_provider_setup_to_document(
        &mut document,
        &base_url,
        api_key.as_str(),
        model_id,
        model_name,
        setup.format,
        setup.provider_id.trim(),
    )?;
    crate::util::config::atomic_write_string(path, &document.to_string())?;
    Ok(())
}

fn document_table_mut<'a>(
    document: &'a mut toml_edit::DocumentMut,
    name: &str,
) -> Result<&'a mut toml_edit::Table, ProviderSetupError> {
    document
        .entry(name)
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or_else(|| ProviderSetupError::Io(format!("[{name}] is not a table")))
}

fn child_table_mut<'a>(
    table: &'a mut toml_edit::Table,
    name: &str,
    path: &str,
) -> Result<&'a mut toml_edit::Table, ProviderSetupError> {
    table
        .entry(name)
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or_else(|| ProviderSetupError::Io(format!("[{path}] is not a table")))
}

fn apply_provider_setup_to_document(
    document: &mut toml_edit::DocumentMut,
    base_url: &str,
    api_key: &str,
    model_id: &str,
    model_name: &str,
    format: SetupWireFormat,
    provider_id: &str,
) -> Result<(), ProviderSetupError> {
    {
        let endpoints = document_table_mut(document, "endpoints")?;
        endpoints.insert("api_base_url", toml_edit::value(base_url));
        endpoints.insert("models_base_url", toml_edit::value(base_url));
        endpoints.insert(
            "models_list_url",
            toml_edit::value(provider_models_list_url(base_url)),
        );
    }
    {
        let models = document_table_mut(document, "models")?;
        models.insert("default", toml_edit::value(model_id));
    }
    // `/setup` never writes `allowed_models`. A leftover list (only setup/Studio-written ids, or
    // one that would block the model just saved) is dropped so every model of the new provider
    // is switchable; a hand-written list that admits the new default is left alone.
    drop_stale_allowlist(document, model_id);
    // Role models still pointing at a Studio model or an earlier setup provider would keep
    // calling a dead endpoint (401s) after the switch; point them at the new default.
    repoint_stale_role_models(document, model_id, base_url);

    {
        let providers = document_table_mut(document, "model_providers")?;
        let provider = child_table_mut(providers, SETUP_PROVIDER_ID, "model_providers.setup")?;
        provider.insert("base_url", toml_edit::value(base_url));
        provider.insert("api_backend", toml_edit::value(format.api_backend()));
        provider.insert(
            "id",
            toml_edit::value(if provider_id.is_empty() {
                CUSTOM_PROVIDER_ID
            } else {
                provider_id
            }),
        );
        if api_key.is_empty() {
            provider.remove("api_key");
        } else {
            provider.insert("api_key", toml_edit::value(api_key));
        }
    }

    let models = document_table_mut(document, "model")?;
    let model = child_table_mut(models, model_id, &format!("model.{model_id}"))?;
    model.insert("model", toml_edit::value(model_id));
    model.insert("base_url", toml_edit::value(base_url));
    model.insert("name", toml_edit::value(model_name));
    model.insert("api_backend", toml_edit::value(format.api_backend()));
    model.insert("auth_scheme", toml_edit::value(format.auth_scheme()));
    model.insert("model_provider", toml_edit::value(SETUP_PROVIDER_ID));
    if format == SetupWireFormat::Anthropic {
        let headers = model
            .entry("extra_headers")
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_like_mut()
            .ok_or_else(|| {
                ProviderSetupError::Io(format!("[model.{model_id}.extra_headers] is not a table"))
            })?;
        headers.insert("anthropic-version", toml_edit::value("2023-06-01"));
    } else if let Some(headers) = model
        .get_mut("extra_headers")
        .and_then(toml_edit::Item::as_table_like_mut)
    {
        // HTTP header names are case-insensitive; retain all unrelated headers.
        let stale: Vec<String> = headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("anthropic-version"))
            .map(|(name, _)| name.to_owned())
            .collect();
        for name in stale {
            headers.remove(&name);
        }
    }
    // `/setup` owns this table's endpoint and credential: drop leftovers that would route the
    // request elsewhere or outrank the key just entered.
    model.remove("api_base_url");
    if api_key.is_empty() {
        model.remove("api_key");
    } else {
        model.insert("api_key", toml_edit::value(api_key));
        model.remove("env_key");
        model.remove("auth_provider");
    }
    Ok(())
}

fn setup_model_ids(document: &toml_edit::DocumentMut) -> std::collections::HashSet<String> {
    document
        .get("model")
        .and_then(toml_edit::Item::as_table_like)
        .map(|models| {
            models
                .iter()
                .filter(|(_, m)| {
                    m.get("model_provider").and_then(toml_edit::Item::as_str)
                        == Some(SETUP_PROVIDER_ID)
                })
                .map(|(id, _)| id.to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Drop `[models] allowed_models` when it is a leftover: every entry is an exact id an earlier
/// setup or Studio wrote, or the list would not admit `new_default` (the model `/setup` just
/// saved). A hand-written list that admits the new default stays, and so do invalid globs.
fn drop_stale_allowlist(document: &mut toml_edit::DocumentMut, new_default: &str) {
    let setup_models = setup_model_ids(document);
    let Some(models) = document
        .get_mut("models")
        .and_then(toml_edit::Item::as_table_like_mut)
    else {
        return;
    };
    let Some(list) = models
        .get("allowed_models")
        .and_then(toml_edit::Item::as_array)
    else {
        return;
    };
    let Some(patterns) = list
        .iter()
        .map(|v| v.as_str().map(str::to_owned))
        .collect::<Option<Vec<String>>>()
    else {
        return;
    };
    if patterns.is_empty() {
        models.remove("allowed_models");
        return;
    }
    let is_glob = |p: &str| p.contains(['*', '?', '[', ']', '{', '}', '!']);
    let tool_written = patterns.iter().all(|p| {
        !is_glob(p)
            && (p == new_default
                || setup_models.contains(p)
                || crate::agent::remote_config::is_studio_model_id(p))
    });
    let blocks_new_default = match crate::agent::remote_config::ModelGlobSet::compile(Some(&patterns))
    {
        Ok(Some(set)) => !set.matches(new_default, new_default),
        _ => false,
    };
    if tool_written || blocks_new_default {
        tracing::info!(?patterns, "/setup: removing stale [models] allowed_models");
        models.remove("allowed_models");
    }
}

/// `[models]` keys that name the model a background feature runs on.
pub(crate) const ROLE_MODEL_KEYS: &[(&str, &str)] = &[
    ("models", "web_search"),
    ("models", "session_summary"),
    ("models", "prompt_suggestion"),
    ("models", "image_description"),
    ("ui", "fork_secondary_model"),
];

/// Point role models at `new_default` when they reference a Studio model (`studio-*`) or a model
/// an earlier `/setup` saved for a different endpoint. A role model the user configured for a
/// model table of their own (own endpoint, not setup-managed) or a built-in id is left alone.
fn repoint_stale_role_models(
    document: &mut toml_edit::DocumentMut,
    new_default: &str,
    base_url: &str,
) {
    let stale: Vec<(&str, &str)> = ROLE_MODEL_KEYS
        .iter()
        .copied()
        .filter(|(table, key)| {
            let Some(value) = document
                .get(table)
                .and_then(|t| t.get(key))
                .and_then(toml_edit::Item::as_str)
                .map(str::trim)
            else {
                return false;
            };
            if value.is_empty() || value == new_default {
                return false;
            }
            if crate::agent::remote_config::is_studio_model_id(value) {
                return true;
            }
            document
                .get("model")
                .and_then(|m| m.get(value))
                .is_some_and(|m| {
                    m.get("model_provider").and_then(toml_edit::Item::as_str)
                        == Some(SETUP_PROVIDER_ID)
                        && m.get("base_url").and_then(toml_edit::Item::as_str) != Some(base_url)
                })
        })
        .collect();
    for (table, key) in stale {
        if let Some(t) = document
            .get_mut(table)
            .and_then(toml_edit::Item::as_table_like_mut)
        {
            tracing::info!(role = %format!("{table}.{key}"), "/setup: pointing role model at the new default");
            t.insert(key, toml_edit::value(new_default));
        }
    }
}

/// Credential written by `/setup` for custom-endpoint catalog fetches.
pub fn provider_setup_api_key_from_toml(config: &toml::Value) -> Option<String> {
    let base_url = config
        .get("endpoints")
        .and_then(|endpoints| {
            ["models_base_url", "api_base_url", "xai_api_base_url"]
                .into_iter()
                .filter_map(|key| endpoints.get(key).and_then(toml::Value::as_str))
                .find(|url| !url.trim().is_empty())
        })
        .or_else(|| {
            config
                .get("model_providers")?
                .get(SETUP_PROVIDER_ID)?
                .get("base_url")?
                .as_str()
        })
        .and_then(|url| normalize_provider_base_url(url).ok());
    config
        .get("model_providers")
        .and_then(|providers| providers.get(SETUP_PROVIDER_ID))
        .and_then(|provider| provider.get("api_key"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty() && is_plausible_provider_api_key(key))
        .map(str::to_owned)
        .or_else(|| {
            config
                .get("model")
                .and_then(toml::Value::as_table)
                .and_then(|models| {
                    models.values().find_map(|entry| {
                        let is_setup_model =
                            entry.get("model_provider").and_then(toml::Value::as_str)
                                == Some(SETUP_PROVIDER_ID);
                        let entry_url = entry.get("base_url").and_then(toml::Value::as_str);
                        let matches_provider = match (base_url.as_deref(), entry_url) {
                            (Some(base_url), Some(entry_url)) => {
                                normalize_provider_base_url(entry_url).ok().as_deref()
                                    == Some(base_url)
                            }
                            _ => is_setup_model,
                        };
                        if !matches_provider {
                            return None;
                        }
                        entry
                            .get("api_key")
                            .and_then(toml::Value::as_str)
                            .map(str::trim)
                            .filter(|key| !key.is_empty() && is_plausible_provider_api_key(key))
                            .map(str::to_owned)
                    })
                })
        })
}

/// Resolve `/format` only from persisted setup, never environment defaults or
/// an arbitrary catalog model. An explicit foreign provider/URL is not ours.
pub fn saved_format_target_from_toml(
    config: &toml::Value,
    active_model: Option<&str>,
) -> Option<ProviderSetup> {
    let provider = config.get("model_providers")?.get(SETUP_PROVIDER_ID)?;
    let base_url = normalize_provider_base_url(provider.get("base_url")?.as_str()?).ok()?;
    let models = config.get("model")?;
    let belongs_to_provider = |id: &str| {
        let Some(model) = models.get(id) else {
            return false;
        };
        let binding = model.get("model_provider").and_then(toml::Value::as_str);
        if binding.is_some_and(|binding| binding != SETUP_PROVIDER_ID) {
            return false;
        }
        match model.get("base_url").and_then(toml::Value::as_str) {
            Some(url) => {
                normalize_provider_base_url(url).ok().as_deref() == Some(base_url.as_str())
            }
            None => binding == Some(SETUP_PROVIDER_ID),
        }
    };
    let model_id = active_model
        .filter(|id| belongs_to_provider(id))
        .or_else(|| {
            config
                .get("models")?
                .get("default")?
                .as_str()
                .filter(|id| belongs_to_provider(id))
        })?;
    if model_id.trim().is_empty() {
        return None;
    }
    let model = models.get(model_id)?;
    // Only this provider or the validated target may supply credentials.
    let api_key = provider
        .get("api_key")
        .or_else(|| model.get("api_key"))
        .and_then(toml::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Some(ProviderSetup {
        base_url,
        api_key,
        model_id: model_id.to_owned(),
        model_name: model
            .get("name")
            .and_then(toml::Value::as_str)
            .unwrap_or(model_id)
            .to_owned(),
        format: model
            .get("api_backend")
            .or_else(|| provider.get("api_backend"))
            .and_then(toml::Value::as_str)
            .map(SetupWireFormat::from_api_backend)
            .unwrap_or_default(),
        provider_id: provider
            .get("id")
            .and_then(toml::Value::as_str)
            .unwrap_or(CUSTOM_PROVIDER_ID)
            .to_owned(),
    })
}

pub fn load_saved_format_target(active_model: Option<&str>) -> Option<ProviderSetup> {
    load_saved_format_target_at(&crate::util::config::user_config_path(), active_model)
}

fn load_saved_format_target_at(path: &Path, active_model: Option<&str>) -> Option<ProviderSetup> {
    let content = crate::util::config::read_to_string_or_empty(path).ok()?;
    let config = toml::from_str::<toml::Value>(&content).ok()?;
    saved_format_target_from_toml(&config, active_model)
}

pub struct ExistingProviderSetup {
    pub base_url: String,
    pub api_key: String,
    pub format: SetupWireFormat,
    pub provider_id: String,
}

pub fn load_existing_provider_setup() -> Option<(String, String)> {
    load_existing_provider_setup_details().map(|setup| (setup.base_url, setup.api_key))
}

/// Credentials for a single `[model.<id>]` entry (base URL + key + format).
fn model_entry_provider_setup(
    parsed: &toml::Value,
    model_id: &str,
) -> Option<ExistingProviderSetup> {
    let model = parsed.get("model")?.get(model_id)?;
    let provider_binding = model
        .get("model_provider")
        .and_then(toml::Value::as_str)
        .map(str::to_owned);
    let provider = provider_binding.as_deref().and_then(|id| {
        parsed.get("model_providers").and_then(|providers| providers.get(id))
    });
    let base_url = model
        .get("base_url")
        .or_else(|| model.get("api_base_url"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            provider?
                .get("base_url")
                .and_then(toml::Value::as_str)
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .map(str::to_owned)
        })?;
    let provider_id = provider_binding
        .clone()
        .unwrap_or_else(|| CUSTOM_PROVIDER_ID.to_owned());
    let api_key = model
        .get("api_key")
        .and_then(toml::Value::as_str)
        .map(sanitize_provider_api_key)
        .filter(|key| !key.is_empty())
        .or_else(|| {
            provider?
                .get("api_key")
                .and_then(toml::Value::as_str)
                .map(sanitize_provider_api_key)
                .filter(|key| !key.is_empty())
        })
        .or_else(|| {
            provider_setup_api_key_from_toml(parsed)
                .map(|key| sanitize_provider_api_key(&key))
                .filter(|key| !key.is_empty())
        })
        .unwrap_or_default();
    let format = model
        .get("api_backend")
        .and_then(toml::Value::as_str)
        .map(SetupWireFormat::from_api_backend)
        .or_else(|| {
            provider?
                .get("api_backend")
                .and_then(toml::Value::as_str)
                .map(SetupWireFormat::from_api_backend)
        })
        .unwrap_or_else(|| setup_wire_format_from_config(parsed));
    Some(ExistingProviderSetup {
        base_url,
        api_key,
        format,
        provider_id,
    })
}

/// First `[model.*]` entry that carries a usable base URL (direct or via its provider).
fn first_model_entry_setup(parsed: &toml::Value) -> Option<ExistingProviderSetup> {
    let models = parsed.get("model")?.as_table()?;
    for id in models.keys() {
        if let Some(setup) = model_entry_provider_setup(parsed, id) {
            return Some(setup);
        }
    }
    None
}

fn first_model_provider_setup(parsed: &toml::Value) -> Option<ExistingProviderSetup> {
    let providers = parsed.get("model_providers")?.as_table()?;
    for (id, provider) in providers {
        let Some(base_url) = provider
            .get("base_url")
            .and_then(toml::Value::as_str)
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_owned)
        else {
            continue;
        };
        let api_key = provider
            .get("api_key")
            .and_then(toml::Value::as_str)
            .map(sanitize_provider_api_key)
            .unwrap_or_default();
        let format = provider
            .get("api_backend")
            .and_then(toml::Value::as_str)
            .map(SetupWireFormat::from_api_backend)
            .unwrap_or_default();
        return Some(ExistingProviderSetup {
            base_url,
            api_key,
            format,
            provider_id: id.clone(),
        });
    }
    None
}

pub fn load_existing_provider_setup_details() -> Option<ExistingProviderSetup> {
    load_existing_provider_setup_details_at(&crate::util::config::user_config_path())
}

/// Path-injectable form of [`load_existing_provider_setup_details`].
fn load_existing_provider_setup_details_at(path: &Path) -> Option<ExistingProviderSetup> {
    let content = crate::util::config::read_to_string_or_empty(path).unwrap_or_default();
    let parsed = if content.trim().is_empty() {
        None
    } else {
        toml::from_str::<toml::Value>(&content).ok()
    };

    if let Some(parsed) = parsed.as_ref() {
        let base_url = parsed
            .get("endpoints")
            .and_then(|endpoints| {
                endpoints
                    .get("models_base_url")
                    .or_else(|| endpoints.get("api_base_url"))
                    .or_else(|| endpoints.get("xai_api_base_url"))
            })
            .and_then(toml::Value::as_str)
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_owned);
        // Only credentials persisted for this provider count. Ambient first-party
        // key variables are not a provider setup: filling one in here would point a
        // launch at a first-party endpoint nobody configured, and would attach an
        // inherited key to whatever base URL happens to be present.
        let api_key = provider_setup_api_key_from_toml(parsed)
            .map(|key| sanitize_provider_api_key(&key))
            .filter(|key| !key.is_empty())
            .unwrap_or_default();
        if let Some(base_url) = base_url {
            return Some(ExistingProviderSetup {
                base_url,
                api_key,
                format: setup_wire_format_from_config(parsed),
                provider_id: setup_profile_id_from_config(parsed)
                    .unwrap_or_else(|| CUSTOM_PROVIDER_ID.to_owned()),
            });
        }
        if let Some(setup) = first_model_provider_setup(parsed) {
            return Some(setup);
        }
        if let Some(setup) = first_model_entry_setup(parsed) {
            return Some(setup);
        }
    }
    None
}

/// Prefer the active model's own endpoint when `/models` is opened mid-session.
pub fn load_existing_provider_setup_for_model(model_id: &str) -> Option<ExistingProviderSetup> {
    load_existing_provider_setup_for_model_at(&crate::util::config::user_config_path(), model_id)
}

fn load_existing_provider_setup_for_model_at(
    path: &Path,
    model_id: &str,
) -> Option<ExistingProviderSetup> {
    let content = crate::util::config::read_to_string_or_empty(path).ok()?;
    if content.trim().is_empty() {
        return load_existing_provider_setup_details_at(path);
    }
    let parsed = toml::from_str::<toml::Value>(&content).ok()?;
    model_entry_provider_setup(&parsed, model_id)
        .or_else(|| load_existing_provider_setup_details_at(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Spawn a local endpoint that records `(path, authorization, body)` of every request.
    async fn recording_endpoint() -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<(String, String, serde_json::Value)>>>,
    ) {
        use axum::{body::Bytes, extract::Request};
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        let app = axum::Router::new().fallback(move |req: Request| {
            let log = log.clone();
            async move {
                let path = req.uri().path().to_owned();
                let auth = req
                    .headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                let body: Bytes = axum::body::to_bytes(req.into_body(), 1 << 20)
                    .await
                    .unwrap_or_default();
                let body = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
                log.lock().unwrap().push((path, auth, body));
                axum::response::Response::builder()
                    .status(401)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"error":{"message":"recorded"}}"#))
                    .unwrap()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}/v1"), seen)
    }

    fn setup(url: &str, key: &str, model: &str) -> ProviderSetup {
        ProviderSetup {
            base_url: url.to_owned(),
            api_key: key.to_owned(),
            model_id: model.to_owned(),
            model_name: model.to_owned(),
            format: SetupWireFormat::Openai,
            provider_id: CUSTOM_PROVIDER_ID.to_owned(),
        }
    }

    #[test]
    fn setup_never_writes_allowlist_and_drops_stale_routing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[model.model-b]\nmodel = \"model-b\"\napi_base_url = \"https://old.example/v1\"\nenv_key = \"OLD_PROVIDER_KEY\"\n",
        )
        .unwrap();
        persist_provider_setup_at(&path, &setup("https://provider-b.example/v1", "sk-provider-b-222", "model-b")).unwrap();
        let raw: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(raw["models"].get("allowed_models").is_none());
        let table = raw["model"]["model-b"].as_table().unwrap();
        assert!(!table.contains_key("api_base_url"));
        assert!(!table.contains_key("env_key"));
        assert_eq!(table["api_key"].as_str(), Some("sk-provider-b-222"));
    }

    #[test]
    fn setup_drops_leftover_setup_allowlist_but_keeps_hand_written_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        persist_provider_setup_at(&path, &setup("https://provider-a.example/v1", "sk-a-111", "model-a")).unwrap();
        // What 1.3.42 could leave behind: only setup-written model ids.
        let text = std::fs::read_to_string(&path).unwrap().replace(
            "[models]\n",
            "[models]\nallowed_models = [\"model-a\"]\n",
        );
        std::fs::write(&path, text).unwrap();
        persist_provider_setup_at(&path, &setup("https://provider-b.example/v1", "sk-b-222", "model-b")).unwrap();
        let raw: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(raw["models"].get("allowed_models").is_none());

        // Hand-written lists that admit the model being saved survive /setup.
        for hand in [r#"["model-*"]"#, r#"["model-c", "my-own-model"]"#] {
            let text = std::fs::read_to_string(&path).unwrap().replace(
                "[models]\n",
                &format!("[models]\nallowed_models = {hand}\n"),
            );
            std::fs::write(&path, text).unwrap();
            persist_provider_setup_at(&path, &setup("https://provider-c.example/v1", "sk-c-333", "model-c")).unwrap();
            let raw: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert!(raw["models"].get("allowed_models").is_some(), "{hand} is hand-written");
            let mut doc: toml::Value = raw.clone();
            doc["models"].as_table_mut().unwrap().remove("allowed_models");
            std::fs::write(&path, toml::to_string(&doc).unwrap()).unwrap();
        }
    }

    const FIELD_DEFAULT: &str = "community/morriszdweck/glm-5.3-flash-free";
    const FIELD_STUDIO: &str = "studio-bkf6jzvemvbu80ni-d56c25911e6255bf";

    /// The config a real user had after Supercharge Studio and then /setup: a Studio model with
    /// a dead key, a /setup default, and every role model plus `allowed_models` on the Studio id.
    fn write_field_config(path: &std::path::Path) {
        persist_provider_setup_at(path, &setup("https://gen.pollinations.example/v1", "sk-poll-111", FIELD_DEFAULT)).unwrap();
        let text = std::fs::read_to_string(path).unwrap().replace(
            "[models]\n",
            &format!(
                "[models]\nweb_search = \"{s}\"\nsession_summary = \"{s}\"\nprompt_suggestion = \"{s}\"\nimage_description = \"{s}\"\nallowed_models = [\"{s}\"]\n",
                s = FIELD_STUDIO
            ),
        );
        let text = format!(
            "{text}\n[model.{s}]\nmodel = \"glm-4-flash\"\nbase_url = \"https://llm-api.studio-upstream.example/v1\"\napi_key = \"sk-dead-quota\"\nname = \"Studio model\"\n\n[ui]\nfork_secondary_model = \"{s}\"\n",
            s = FIELD_STUDIO
        );
        std::fs::write(path, text).unwrap();
    }

    fn field_catalog(
        cfg: &crate::agent::config::Config,
    ) -> indexmap::IndexMap<String, crate::agent::config::ModelEntry> {
        let listed = crate::agent::config::resolve_model_list(cfg, None);
        let template = listed.get(FIELD_DEFAULT).unwrap().clone();
        let mut prefetched = listed.clone();
        for id in ["fast-model", "large-model"] {
            let mut entry = template.clone();
            entry.info.model = id.to_owned();
            prefetched.insert(id.to_owned(), entry);
        }
        crate::agent::remote_config::resolve_model_catalog(cfg, Some(prefetched))
    }

    /// Regression (1.3.44 field report): a Studio-only `allowed_models` locked every /setup model
    /// out and sessions ran on the dead Studio model. On load the list is ignored, the default is
    /// used, and role models resolve to the default; the next /setup removes the list and repoints
    /// every role model.
    #[test]
    fn studio_leftovers_never_lock_out_or_hijack_the_setup_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write_field_config(&path);

        // On load, before any /setup.
        let raw: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let cfg = crate::agent::config::Config::new_from_toml_cfg(&raw).unwrap();
        assert!(crate::agent::remote_config::stale_user_allowlist_reason(&cfg, cfg.models.allowed_models.as_deref().unwrap()).is_some());
        let catalog = field_catalog(&cfg);
        for id in [FIELD_DEFAULT, "fast-model", "large-model"] {
            assert!(catalog.get(id).is_some_and(|e| e.info.user_selectable), "{id} must be switchable");
        }
        assert!(crate::agent::remote_config::validate_selectable(&cfg, &catalog).is_ok());
        let (picked, _, _) = crate::agent::remote_config::resolve_default_model(&cfg, &catalog, false);
        assert_eq!(picked, FIELD_DEFAULT, "the session must run on models.default");
        let roles = crate::config::ModelOverrideConfig::resolve(None, None, &raw, None);
        assert_eq!(roles.web_search, FIELD_DEFAULT);
        assert_eq!(roles.session_summary.as_deref(), Some(FIELD_DEFAULT));
        assert_eq!(roles.image_description.as_deref(), Some(FIELD_DEFAULT));
        assert_eq!(roles.prompt_suggestion, crate::config::PromptSuggestModelPin::Pinned(FIELD_DEFAULT.to_owned()));

        // /setup again (same provider) and then a provider switch: list gone, roles repointed.
        for (url, model) in [("https://gen.pollinations.example/v1", FIELD_DEFAULT), ("https://provider-b.example/v1", "model-b1")] {
            if model != FIELD_DEFAULT {
                std::fs::remove_file(&path).unwrap();
                write_field_config(&path);
            }
            persist_provider_setup_at(&path, &setup(url, "sk-new-222", model)).unwrap();
            let raw: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert!(raw["models"].get("allowed_models").is_none(), "{model}");
            for key in ["web_search", "session_summary", "prompt_suggestion", "image_description"] {
                assert_eq!(raw["models"][key].as_str(), Some(model), "models.{key}");
            }
            assert_eq!(raw["ui"]["fork_secondary_model"].as_str(), Some(model));
            assert_eq!(raw["models"]["default"].as_str(), Some(model));
            let cfg = crate::agent::config::Config::new_from_toml_cfg(&raw).unwrap();
            let listed = crate::agent::config::resolve_model_list(&cfg, None);
            let (picked, _, _) = crate::agent::remote_config::resolve_default_model(&cfg, &crate::agent::remote_config::resolve_model_catalog(&cfg, Some(listed)), false);
            assert_eq!(picked, model);
        }
    }

    /// Hand-written lists that admit the default are policy and stay enforced, on load and
    /// across /setup; a list that would block the default is ignored.
    #[test]
    fn hand_written_allowlist_that_admits_the_default_is_still_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        persist_provider_setup_at(&path, &setup("https://gen.pollinations.example/v1", "sk-poll-111", FIELD_DEFAULT)).unwrap();
        let base = std::fs::read_to_string(&path).unwrap();
        for hand in [
            format!(r#"["{FIELD_DEFAULT}", "my-own-model"]"#),
            r#"["community/*"]"#.to_owned(),
        ] {
            let text = base.replace("[models]\n", &format!("[models]\nallowed_models = {hand}\n"));
            std::fs::write(&path, &text).unwrap();
            let raw: toml::Value = toml::from_str(&text).unwrap();
            let cfg = crate::agent::config::Config::new_from_toml_cfg(&raw).unwrap();
            assert_eq!(crate::agent::remote_config::stale_user_allowlist_reason(&cfg, cfg.models.allowed_models.as_deref().unwrap()), None, "{hand}");
            let catalog = field_catalog(&cfg);
            assert!(catalog[FIELD_DEFAULT].info.user_selectable, "{hand}");
            assert!(!catalog["fast-model"].info.user_selectable, "{hand} must still block fast-model");
            assert!(!catalog["large-model"].info.user_selectable, "{hand} must still block large-model");
            let (picked, _, _) = crate::agent::remote_config::resolve_default_model(&cfg, &catalog, false);
            assert_eq!(picked, FIELD_DEFAULT);
            // Re-running /setup for the same default keeps the policy.
            persist_provider_setup_at(&path, &setup("https://gen.pollinations.example/v1", "sk-poll-222", FIELD_DEFAULT)).unwrap();
            let raw: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert!(raw["models"].get("allowed_models").is_some(), "{hand} is hand-written");
        }
        // A list that blocks the default is not a usable policy: ignored on load.
        let text = base.replace("[models]\n", "[models]\nallowed_models = [\"my-own-model\"]\n");
        let raw: toml::Value = toml::from_str(&text).unwrap();
        let cfg = crate::agent::config::Config::new_from_toml_cfg(&raw).unwrap();
        let catalog = field_catalog(&cfg);
        assert!(catalog.values().all(|e| e.info.user_selectable));
    }

    /// Regression: setup A, then setup B; every model B's /models lists must be switchable,
    /// including when a leftover setup-written allowlist is still on disk.
    #[test]
    fn after_switching_provider_every_listed_model_is_switchable() {
        use indexmap::IndexMap;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        persist_provider_setup_at(&path, &setup("https://provider-a.example/v1", "sk-a-111", "model-a")).unwrap();
        persist_provider_setup_at(&path, &setup("https://provider-b.example/v1", "sk-b-222", "model-b1")).unwrap();
        let clean = std::fs::read_to_string(&path).unwrap();
        // Leftover from 1.3.42 on load (written behind setup's back, so no migration ran).
        let leftover = clean.replace(
            "[models]\n",
            "[models]\nallowed_models = [\"model-a\", \"model-b1\"]\n",
        );
        for (label, text) in [("clean", clean.as_str()), ("leftover", leftover.as_str())] {
            let raw: toml::Value = toml::from_str(text).unwrap();
            let cfg = crate::agent::config::Config::new_from_toml_cfg(&raw).unwrap();
            // B's /models lists more than the one model setup saved.
            let listed = crate::agent::config::resolve_model_list(&cfg, None);
            let template = listed.get("model-b1").unwrap().clone();
            let mut prefetched: IndexMap<String, crate::agent::config::ModelEntry> = IndexMap::new();
            for id in ["model-b1", "model-b2", "model-b3"] {
                let mut entry = template.clone();
                entry.info.model = id.to_owned();
                prefetched.insert(id.to_owned(), entry);
            }
            let catalog = crate::agent::remote_config::resolve_model_catalog(&cfg, Some(prefetched));
            for id in ["model-b1", "model-b2", "model-b3"] {
                assert!(
                    catalog.get(id).is_some_and(|e| e.info.user_selectable),
                    "{label}: {id} must be switchable"
                );
            }
            assert!(crate::agent::remote_config::validate_selectable(&cfg, &catalog).is_ok());
        }
        // A hand-written list is still enforced, and the error says how to fix it.
        let mut raw: toml::Value = toml::from_str(&clean).unwrap();
        raw["models"].as_table_mut().unwrap().insert(
            "allowed_models".to_owned(),
            toml::Value::Array(vec!["model-b1".into(), "other".into()]),
        );
        let cfg = crate::agent::config::Config::new_from_toml_cfg(&raw).unwrap();
        let msg = crate::agent::remote_config::allowlist_denied_message(&cfg);
        assert!(msg.contains("allowed_models") && msg.contains("[models]") && msg.contains("config.toml"), "{msg}");
    }

    /// Regression: a second `/setup` that switches provider A -> B (different key) must make the
    /// next request, after a full restart (config re-read from disk), use B's key, base URL and model.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn second_setup_switching_provider_sends_new_key_url_and_model() {
        let (url_a, seen_a) = recording_endpoint().await;
        let (url_b, seen_b) = recording_endpoint().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for (url, key, model, format) in [
            (&url_a, "sk-provider-a-111", "model-a", SetupWireFormat::Anthropic),
            (&url_b, "sk-provider-b-222", "model-b", SetupWireFormat::Openai),
        ] {
            persist_provider_setup_at(
                &path,
                &ProviderSetup {
                    base_url: url.clone(),
                    api_key: key.to_owned(),
                    model_id: model.to_owned(),
                    model_name: model.to_owned(),
                    format,
                    provider_id: CUSTOM_PROVIDER_ID.to_owned(),
                },
            )
            .unwrap();
        }
        let raw: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            provider_setup_api_key_from_toml(&raw).as_deref(),
            Some("sk-provider-b-222")
        );
        let cfg = crate::agent::config::Config::new_from_toml_cfg(&raw).unwrap();
        let default = cfg.models.default.clone().expect("setup writes [models] default");
        assert_eq!(default, "model-b");
        let catalog = crate::agent::config::resolve_model_list(&cfg, None);
        let entry = crate::agent::config::find_model_by_id(&catalog, &default).unwrap();
        let creds = crate::agent::config::resolve_credentials(entry, None);
        assert_eq!(creds.api_key.as_deref(), Some("sk-provider-b-222"));
        assert_eq!(creds.base_url, url_b);
        assert!(
            !entry.extra_headers.keys().any(|h| h.eq_ignore_ascii_case("anthropic-version")),
            "provider A's Messages headers must not stick to provider B"
        );
        let sampler = crate::agent::config::sampling_config_for_model(entry, creds, None, None, None, None);
        let client = supercharge_sampler::SamplingClient::new(sampler).unwrap();
        let request: supercharge_sampling_types::types::ChatCompletionRequest =
            serde_json::from_value(json!({"messages": [{"role": "user", "content": "hi"}]})).unwrap();
        let _ = client.chat_completion(request).await;

        assert!(seen_a.lock().unwrap().is_empty(), "nothing may reach provider A");
        let seen = seen_b.lock().unwrap();
        let (req_path, auth, body) = seen.first().expect("request reached provider B");
        assert!(req_path.starts_with("/v1/"), "{req_path}");
        assert_eq!(auth, "Bearer sk-provider-b-222");
        assert_eq!(body["model"], "model-b");
    }

    fn with_models_server<T>(
        response: fn(&str) -> (&'static str, &'static str),
        fetch: impl FnOnce(&str) -> T,
    ) -> (T, Vec<String>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::mpsc;
        use std::time::Duration;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (stop, stopped) = mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            loop {
                match stopped.try_recv() {
                    Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("accept model request: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 1024];
                while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).unwrap();
                    assert!(read > 0, "request ended before headers");
                    bytes.extend_from_slice(&buffer[..read]);
                    assert!(bytes.len() < 32 * 1024, "request headers too large");
                }
                let request = String::from_utf8(bytes).unwrap().to_ascii_lowercase();
                let (status, body) = response(&request);
                requests.push(request);
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            requests
        });
        let result = fetch(&base_url);
        stop.send(()).unwrap();
        (result, server.join().unwrap())
    }

    fn fetch_saved_models(
        base_url: &str,
        format: SetupWireFormat,
    ) -> Result<Vec<ProviderModel>, ProviderSetupError> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        persist_provider_setup_at(
            &path,
            &ProviderSetup {
                base_url: base_url.to_owned(),
                api_key: "saved-key".to_owned(),
                model_id: "previous-model".to_owned(),
                model_name: "Previous model".to_owned(),
                format,
                provider_id: "openai".to_owned(),
            },
        )
        .unwrap();
        let config: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let key = provider_setup_api_key_from_toml(&config).unwrap();
        fetch_provider_models(
            config["endpoints"]["models_base_url"].as_str().unwrap(),
            &key,
            setup_wire_format_from_config(&config),
            setup_profile_id_from_config(&config).as_deref(),
        )
    }

    #[test]
    fn wire_formats_preserve_aliases_and_backend_serialization() {
        for (format, legacy_name, backend_name, display_name, auth_scheme, backend) in [
            (
                SetupWireFormat::Openai,
                "openai",
                "chat_completions",
                "Chat Completions",
                "bearer",
                ApiBackend::ChatCompletions,
            ),
            (
                SetupWireFormat::Anthropic,
                "anthropic",
                "messages",
                "Messages",
                "x_api_key",
                ApiBackend::Messages,
            ),
            (
                SetupWireFormat::Responses,
                "responses",
                "responses",
                "Responses",
                "bearer",
                ApiBackend::Responses,
            ),
        ] {
            assert_eq!(format.as_str(), legacy_name);
            assert_eq!(format.api_backend(), backend_name);
            assert_eq!(format.display_name(), display_name);
            assert_eq!(format.auth_scheme(), auth_scheme);
            for alias in [legacy_name, backend_name] {
                assert_eq!(SetupWireFormat::parse(alias), format);
                assert_eq!(SetupWireFormat::parse(&alias.to_ascii_uppercase()), format);
                assert_eq!(SetupWireFormat::from_api_backend(alias), format);
            }
            assert_eq!(serde_json::to_value(&backend).unwrap(), json!(backend_name));
            assert_eq!(
                serde_json::from_value::<ApiBackend>(json!(format.api_backend())).unwrap(),
                backend
            );
        }
        assert_eq!(SetupWireFormat::default(), SetupWireFormat::Openai);
        assert_eq!(SetupWireFormat::parse("unknown"), SetupWireFormat::Openai);
    }

    #[test]
    #[serial_test::serial]
    fn provider_setup_prefill_never_invents_a_provider_from_ambient_keys() {
        use supercharge_test_support::EnvGuard;
        let _xai = EnvGuard::set("XAI_API_KEY", "ambient-first-party-key");
        let _grok = EnvGuard::set("GROK_API_KEY", "ambient-grok-key");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        assert!(
            load_existing_provider_setup_details_at(&path).is_none(),
            "an unconfigured machine must not produce a provider setup, even with \
             XAI_API_KEY/GROK_API_KEY present: that would pre-fill /setup with an \
             api.x.ai endpoint nobody chose"
        );

        // A saved provider keeps its own key; the ambient variable is not substituted.
        persist_provider_setup_at(
            &path,
            &ProviderSetup {
                base_url: "https://inference.example.com/v1".to_owned(),
                api_key: "saved-provider-key".to_owned(),
                model_id: "saved-model".to_owned(),
                model_name: "Saved model".to_owned(),
                format: SetupWireFormat::Openai,
                provider_id: "openai".to_owned(),
            },
        )
        .unwrap();
        let existing = load_existing_provider_setup_details_at(&path).expect("saved setup");
        assert_eq!(existing.api_key, "saved-provider-key");
        assert_eq!(existing.base_url, "https://inference.example.com/v1");
        assert_ne!(existing.base_url, "https://api.x.ai/v1");
    }

    #[test]
    fn live_responses_models_use_saved_bearer_key_and_models_endpoint() {
        let (models, requests) = with_models_server(
            |_| ("200 OK", r#"{"data":[{"id":"responses-model"}]}"#),
            |base_url| fetch_saved_models(base_url, SetupWireFormat::Responses),
        );
        assert_eq!(models.unwrap()[0].id, "responses-model");
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("get /v1/models http/1.1\r\n"));
        assert!(requests[0].contains("authorization: bearer saved-key\r\n"));
        assert!(!requests[0].contains("x-api-key:"));
        assert!(!requests[0].contains("anthropic-version:"));
    }

    #[test]
    fn catalog_preserves_all_three_wire_formats() {
        for (format, backend, auth_scheme) in [
            (
                SetupWireFormat::Openai,
                ApiBackend::ChatCompletions,
                AuthScheme::Bearer,
            ),
            (
                SetupWireFormat::Anthropic,
                ApiBackend::Messages,
                AuthScheme::XApiKey,
            ),
            (
                SetupWireFormat::Responses,
                ApiBackend::Responses,
                AuthScheme::Bearer,
            ),
        ] {
            let catalog = provider_models_to_catalog(
                vec![ProviderModel {
                    id: "test-model".to_owned(),
                    name: "Test Model".to_owned(),
                    context_window: None,
                    supports_thinking: false,
                }],
                "https://api.example.com/v1",
                " test-key ",
                format,
            );
            let entry = catalog.get("test-model").unwrap();
            assert_eq!(entry.info.api_backend, backend);
            assert_eq!(entry.info.auth_scheme, auth_scheme);
            assert_eq!(entry.api_key.as_deref(), Some("test-key"));
            assert_eq!(
                entry
                    .info
                    .extra_headers
                    .get("anthropic-version")
                    .map(String::as_str),
                (format == SetupWireFormat::Anthropic).then_some("2023-06-01")
            );
        }
    }

    fn format_target_config() -> toml::Value {
        toml::from_str(
            r#"
            [models]
            default = "saved"
            [model_providers.setup]
            base_url = "https://api.example.com/v1"
            api_key = "saved-key"
            api_backend = "messages"
            id = "custom"
            [model.saved]
            model_provider = "setup"
            name = "Saved Model"
            [model.active]
            base_url = "https://api.example.com/v1/"
            [model.foreign]
            model_provider = "other"
            base_url = "https://api.example.com/v1"
            api_key = "foreign-key"
            [model.stale]
            model_provider = "setup"
            base_url = "https://other.example.com/v1"
        "#,
        )
        .unwrap()
    }

    #[test]
    fn saved_format_target_loads_real_config_document_and_model_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(load_saved_format_target_at(&path, None).is_none());
        let mut config = format_target_config();
        config["model"]["saved"].as_table_mut().unwrap().insert(
            "api_backend".to_owned(),
            toml::Value::String("responses".to_owned()),
        );
        std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
        let setup = load_saved_format_target_at(&path, Some("foreign")).unwrap();
        assert_eq!(setup.model_id, "saved");
        assert_eq!(setup.format, SetupWireFormat::Responses);
        assert_eq!(setup.api_key, "saved-key");
        persist_provider_setup_at(&path, &setup).unwrap();
        let setup = load_saved_format_target_at(&path, Some("saved")).unwrap();
        assert_eq!(setup.format, SetupWireFormat::Responses);
        assert_eq!(setup.model_name, "Saved Model");
        std::fs::write(&path, "[invalid").unwrap();
        assert!(load_saved_format_target_at(&path, Some("saved")).is_none());
        std::fs::write(&path, "").unwrap();
        assert!(load_saved_format_target_at(&path, Some("saved")).is_none());
    }

    #[test]
    fn saved_format_target_uses_only_provider_models() {
        let config = format_target_config();
        for (active, expected) in [
            (Some("active"), "active"),
            (Some("saved"), "saved"),
            (Some("foreign"), "saved"),
            (Some("stale"), "saved"),
            (Some("catalog-only"), "saved"),
            (None, "saved"),
        ] {
            let target = saved_format_target_from_toml(&config, active).unwrap();
            assert_eq!(target.model_id, expected);
            assert_eq!(target.api_key, "saved-key");
            assert_eq!(target.base_url, "https://api.example.com/v1");
            assert_eq!(target.format, SetupWireFormat::Anthropic);
            assert_eq!(target.provider_id, "custom");
        }
        assert_eq!(
            saved_format_target_from_toml(&config, None)
                .unwrap()
                .model_name,
            "Saved Model"
        );
    }

    #[test]
    fn saved_format_target_requires_saved_setup_and_safe_default() {
        assert!(
            saved_format_target_from_toml(&toml::Value::Table(Default::default()), Some("active"))
                .is_none()
        );
        let mut config = format_target_config();
        config["models"]["default"] = toml::Value::String("foreign".to_owned());
        assert!(saved_format_target_from_toml(&config, None).is_none());
        assert!(saved_format_target_from_toml(&config, Some("stale")).is_none());
        assert_eq!(
            saved_format_target_from_toml(&config, Some("active"))
                .unwrap()
                .model_id,
            "active"
        );
        config["model_providers"]["setup"]["base_url"] =
            toml::Value::String("not a URL".to_owned());
        assert!(saved_format_target_from_toml(&config, Some("active")).is_none());
    }

    #[test]
    fn sequential_format_persistence_preserves_credentials_and_unrelated_settings() {
        for inline_headers in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            let config = format_target_config();
            let mut document = toml::to_string(&config)
                .unwrap()
                .parse::<toml_edit::DocumentMut>()
                .unwrap();
            document["model"]["saved"]["temperature"] = toml_edit::value(0.4);
            if inline_headers {
                let mut headers = toml_edit::InlineTable::new();
                headers.insert("X-Tenant", "tenant".into());
                headers.insert("Anthropic-Version", "old".into());
                document["model"]["saved"]["extra_headers"] = toml_edit::value(headers);
            } else {
                document["model"]["saved"]["extra_headers"]["X-Tenant"] =
                    toml_edit::value("tenant");
            }
            std::fs::write(&path, document.to_string()).unwrap();
            for format in [
                SetupWireFormat::Anthropic,
                SetupWireFormat::Responses,
                SetupWireFormat::Anthropic,
                SetupWireFormat::Openai,
            ] {
                let before: toml::Value =
                    toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                let mut target = saved_format_target_from_toml(&before, Some("foreign")).unwrap();
                target.format = format;
                persist_provider_setup_at(&path, &target).unwrap();
                let after: toml::Value =
                    toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                let restored = saved_format_target_from_toml(&after, Some("saved")).unwrap();
                assert_eq!(restored.format, format);
                assert_eq!(restored.api_key, "saved-key");
                assert_eq!(restored.model_id, "saved");
                assert_eq!(restored.model_name, "Saved Model");
                assert_eq!(after["model"]["foreign"], before["model"]["foreign"]);
                assert_eq!(after["model"]["saved"]["temperature"].as_float(), Some(0.4));
                assert_eq!(
                    after["model"]["saved"]["auth_scheme"].as_str(),
                    Some(format.auth_scheme())
                );
                assert_eq!(
                    after["model"]["saved"]["api_backend"].as_str(),
                    Some(format.api_backend())
                );
                let headers = after["model"]["saved"]["extra_headers"].as_table().unwrap();
                assert_eq!(headers["X-Tenant"].as_str(), Some("tenant"));
                assert_eq!(
                    headers
                        .keys()
                        .any(|name| name.eq_ignore_ascii_case("anthropic-version")),
                    format == SetupWireFormat::Anthropic
                );
            }
        }
    }

    #[test]
    fn persistence_round_trips_all_three_wire_formats() {
        for format in [
            SetupWireFormat::Openai,
            SetupWireFormat::Anthropic,
            SetupWireFormat::Responses,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            persist_provider_setup_at(
                &path,
                &ProviderSetup {
                    base_url: "https://api.example.com/v1".to_owned(),
                    api_key: "test-key".to_owned(),
                    model_id: "test-model".to_owned(),
                    model_name: "Test Model".to_owned(),
                    format,
                    provider_id: CUSTOM_PROVIDER_ID.to_owned(),
                },
            )
            .unwrap();
            let parsed: toml::Value =
                toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            assert_eq!(
                parsed["model_providers"]["setup"]["api_backend"].as_str(),
                Some(format.api_backend())
            );
            assert_eq!(
                parsed["model"]["test-model"]["api_backend"].as_str(),
                Some(format.api_backend())
            );
            assert_eq!(
                parsed["model"]["test-model"]["auth_scheme"].as_str(),
                Some(format.auth_scheme())
            );
            assert_eq!(
                parsed["endpoints"]["models_list_url"].as_str(),
                Some("https://api.example.com/v1/models")
            );
            assert_eq!(setup_wire_format_from_config(&parsed), format);
            assert_eq!(first_model_provider_setup(&parsed).unwrap().format, format);
        }
    }

    #[test]
    fn saved_responses_backends_restore_without_downgrade() {
        let parsed: toml::Value = toml::from_str(
            r#"
            [endpoints]
            models_base_url = "https://api.example.com/v1"
            [model_providers.setup]
            base_url = "https://api.example.com/v1"
            api_backend = "responses"
            id = "custom"
            "#,
        )
        .unwrap();
        assert_eq!(
            setup_wire_format_from_config(&parsed),
            SetupWireFormat::Responses
        );
        assert_eq!(
            first_model_provider_setup(&parsed).unwrap().format,
            SetupWireFormat::Responses
        );

        let parsed: toml::Value = toml::from_str(
            r#"
            [model_providers.saved]
            base_url = "https://api.example.com/v1"
            api_backend = "responses"
            "#,
        )
        .unwrap();
        let restored = first_model_provider_setup(&parsed).unwrap();
        assert_eq!(restored.format, SetupWireFormat::Responses);
        assert_eq!(restored.base_url, "https://api.example.com/v1");
        assert_eq!(restored.provider_id, "saved");
        assert!(restored.api_key.is_empty());
    }

    #[test]
    fn live_models_use_saved_bearer_key_without_anonymous_union() {
        let (models, requests) = with_models_server(
            |request| {
                if request.contains("authorization: bearer saved-key\r\n") {
                    ("200 OK", r#"{"data":[{"id":"private-model"}]}"#)
                } else {
                    ("200 OK", r#"{"data":[{"id":"public-only-model"}]}"#)
                }
            },
            |base_url| fetch_saved_models(base_url, SetupWireFormat::Openai),
        );
        let models = models.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "private-model");
        assert_eq!(requests.len(), 1, "must not probe anonymously first");
        assert!(requests[0].starts_with("get /v1/models http/1.1\r\n"));
        assert!(requests[0].contains("authorization: bearer saved-key\r\n"));
    }

    #[test]
    fn live_models_use_saved_anthropic_key_and_version() {
        let (models, requests) = with_models_server(
            |_| {
                (
                    "200 OK",
                    r#"{"data":[{"id":"claude-test","display_name":"Claude Test"}]}"#,
                )
            },
            |base_url| fetch_saved_models(base_url, SetupWireFormat::Anthropic),
        );
        assert_eq!(models.unwrap()[0].name, "Claude Test");
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("x-api-key: saved-key\r\n"));
        assert!(requests[0].contains("anthropic-version: 2023-06-01\r\n"));
        assert!(!requests[0].contains("authorization:"));
    }

    #[test]
    fn live_models_allow_anonymous_requests_only_without_key() {
        let (models, requests) = with_models_server(
            |_| ("200 OK", r#"{"data":[{"id":"local-model"}]}"#),
            |base_url| {
                fetch_provider_models(base_url, "  ", SetupWireFormat::Openai, Some("ollama"))
            },
        );
        assert_eq!(models.unwrap()[0].id, "local-model");
        assert_eq!(requests.len(), 1);
        assert!(!requests[0].contains("authorization:"));
        assert!(!requests[0].contains("x-api-key:"));
    }

    #[test]
    fn live_models_surface_http_errors_without_curated_fallback() {
        for key in ["saved-key", ""] {
            let (result, requests) = with_models_server(
                |request| {
                    if request.contains("authorization:") {
                        ("401 Unauthorized", r#"{"error":"invalid key"}"#)
                    } else {
                        ("403 Forbidden", r#"{"error":"key required"}"#)
                    }
                },
                |base_url| {
                    fetch_provider_models(base_url, key, SetupWireFormat::Openai, Some("openai"))
                },
            );
            let error = result.unwrap_err().to_string();
            assert!(error.contains(if key.is_empty() {
                "HTTP 403"
            } else {
                "HTTP 401"
            }));
            assert!(error.contains(if key.is_empty() {
                "key required"
            } else {
                "invalid key"
            }));
            assert_eq!(requests.len(), 1);
        }
    }

    #[test]
    fn live_models_surface_empty_catalog_without_curated_fallback() {
        for key in ["saved-key", ""] {
            let (result, requests) = with_models_server(
                |_| ("200 OK", r#"{"data":[]}"#),
                |base_url| {
                    fetch_provider_models(base_url, key, SetupWireFormat::Openai, Some("openai"))
                },
            );
            assert_eq!(
                result.unwrap_err(),
                ProviderSetupError::Fetch("provider returned no models".to_owned())
            );
            assert_eq!(requests.len(), 1);
        }
    }

    #[test]
    fn provider_models_to_catalog_uses_setup_auth_and_backend() {
        let models = vec![
            ProviderModel {
                id: "kimi-code".to_owned(),
                name: "Kimi Code".to_owned(),
                context_window: None,
                supports_thinking: false,
            },
            ProviderModel {
                id: "openai".to_owned(),
                name: "openai".to_owned(),
                context_window: None,
                supports_thinking: false,
            },
        ];
        let catalog = provider_models_to_catalog(
            models,
            "https://gen.pollinations.ai/v1",
            "test-key",
            SetupWireFormat::Openai,
        );
        assert_eq!(catalog.len(), 2);
        let entry = catalog.get("kimi-code").expect("kimi-code");
        assert_eq!(entry.name.as_deref(), Some("Kimi Code"));
        assert_eq!(entry.info.api_backend, ApiBackend::ChatCompletions);
        assert_eq!(entry.info.auth_scheme, AuthScheme::Bearer);
        assert_eq!(entry.api_key.as_deref(), Some("test-key"));
    }

    #[test]
    fn normalizes_https_and_strips_models_suffix() {
        assert_eq!(
            normalize_provider_base_url("https://api.openai.com/v1/models").unwrap(),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_provider_base_url("api.openai.com/v1").unwrap(),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_provider_base_url("http://127.0.0.1:11434/v1/").unwrap(),
            "http://127.0.0.1:11434/v1"
        );
    }

    #[test]
    fn rejects_insecure_remote_http() {
        assert_eq!(
            normalize_provider_base_url("http://example.com/v1").unwrap_err(),
            ProviderSetupError::InsecureHttp
        );
    }

    #[test]
    fn reasoning_effort_ids_by_family() {
        assert_eq!(reasoning_effort_ids_for_model("gpt-6-astra"), &["xhigh", "high", "medium", "low"]);
        assert_eq!(reasoning_effort_ids_for_model("gpt-5"), &["high", "medium", "low"]);
        assert_eq!(reasoning_effort_ids_for_model("o4-mini"), &["high", "medium", "low"]);
        assert!(reasoning_effort_ids_for_model("gpt-4o").is_empty());
    }

    #[test]
    fn infers_thinking_for_current_families() {
        for id in [
            "gpt-6-astra", "gpt-5.2", "openai/gpt-5-mini", "o1", "o3-pro", "o4-mini",
            "claude-3-7-sonnet-20250219", "claude-sonnet-4-5", "claude-opus-4-1",
            "gemini-2.5-pro", "gemini-3-flash", "deepseek-r1", "deepseek-reasoner",
            "qwq-32b", "qwen3-235b-a22b", "gpt-oss-120b", "my-thinking-model",
        ] {
            assert!(infer_thinking_from_id(id), "{id} should infer thinking");
        }
        for id in [
            "gpt-4o", "gpt-4.1-mini", "claude-3-5-sonnet", "gemini-2.0-flash",
            "deepseek-chat", "llama-3.3-70b", "text-embedding-3-large",
        ] {
            assert!(!infer_thinking_from_id(id), "{id} should not infer thinking");
        }
    }

    #[test]
    fn metadata_thinking_wins_over_name() {
        use serde_json::json;
        assert_eq!(model_metadata_thinking(&json!({"supported_parameters":["tools","reasoning","include_reasoning"]})), Some(true));
        assert_eq!(model_metadata_thinking(&json!({"supported_parameters":["tools","temperature"]})), Some(false));
        assert_eq!(model_metadata_thinking(&json!({"capabilities":{"reasoning":true}})), Some(true));
        assert_eq!(model_metadata_thinking(&json!({"capabilities":{"thinking":false}})), Some(false));
        assert_eq!(model_metadata_thinking(&json!({"thinking":{"supported":true}})), Some(true));
        assert_eq!(model_metadata_thinking(&json!({"features":["vision","reasoning"]})), Some(true));
        assert_eq!(model_metadata_thinking(&json!({"id":"x"})), None);
        assert!(model_entry_supports_thinking(&json!({"id":"gpt-6-astra"}), "gpt-6-astra", ""));
        assert!(!model_entry_supports_thinking(&json!({"supported_parameters":["tools"]}), "o3", ""));
    }

    #[test]
    fn skips_embedding_and_audio_models() {
        let body = json!({
            "data": [
                {"id": "gpt-4o"},
                {"id": "text-embedding-3-large"},
                {"id": "whisper-1"},
            ]
        });
        let models = parse_openai_models_list(&body);
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["gpt-4o"]
        );
    }

    #[test]
    fn parses_openai_model_list() {
        let body = json!({
            "object": "list",
            "data": [
                {"id": "gpt-4o", "object": "model"},
                {"id": "o3-mini", "name": "o3 mini"},
                {"object": "model"},
            ]
        });
        let models = parse_openai_models_list(&body);
        assert_eq!(
            models,
            vec![
                ProviderModel {
                    id: "gpt-4o".to_owned(),
                    name: "gpt-4o".to_owned(),
                    context_window: None,
                    supports_thinking: false,
                },
                ProviderModel {
                    id: "o3-mini".to_owned(),
                    name: "o3 mini".to_owned(),
                    context_window: None,
                    supports_thinking: true,
                },
            ]
        );
    }

    #[test]
    fn parses_title_as_display_name_fallback() {
        // parse_openai_models_list sorts by id, so assert by id → name map.
        let models = parse_openai_models_list(&json!({
            "data": [
                {"id": "pollinations/foo", "title": " Foo Title "},
                {"id": "pollinations/bar", "display_name": "Bar", "title": "Ignored"},
                {"id": "pollinations/baz", "title": " ", "name": "Baz Name"}
            ]
        }));
        let by_id: std::collections::BTreeMap<_, _> = models
            .into_iter()
            .map(|m| (m.id, m.name))
            .collect();
        assert_eq!(by_id.get("pollinations/foo").map(String::as_str), Some("Foo Title"));
        assert_eq!(by_id.get("pollinations/bar").map(String::as_str), Some("Bar"));
        assert_eq!(by_id.get("pollinations/baz").map(String::as_str), Some("Baz Name"));
    }

    #[test]
    fn parses_anthropic_display_names_with_name_and_id_fallbacks() {
        let models = parse_openai_models_list(&json!({
            "data": [
                {"id": "a", "display_name": " Claude A ", "name": "legacy"},
                {"id": "b", "display_name": " ", "name": "Claude B"},
                {"id": "c", "display_name": null, "name": " "}
            ]
        }));
        assert_eq!(
            models
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Claude A", "Claude B", "c"]
        );
    }


    #[test]
    fn load_existing_falls_back_to_model_entry_base_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
            [model.my-custom]
            model = "my-custom"
            base_url = "https://custom.example/v1"
            api_key = "sk-custom-key-1234567890"
            api_backend = "responses"
            "#,
        )
        .unwrap();
        let setup = load_existing_provider_setup_details_at(&path).expect("model entry setup");
        assert_eq!(setup.base_url, "https://custom.example/v1");
        assert_eq!(setup.api_key, "sk-custom-key-1234567890");
        assert_eq!(setup.format, SetupWireFormat::Responses);
        let for_model =
            load_existing_provider_setup_for_model_at(&path, "my-custom").expect("for model");
        assert_eq!(for_model.base_url, "https://custom.example/v1");
    }

    #[test]
    fn setup_key_fallback_ignores_unrelated_models() {
        let config: toml::Value = toml::from_str(
            r#"
            [endpoints]
            models_base_url = "https://api.example.com/v1"
            [model.a_unrelated]
            base_url = "https://other.example.com/v1"
            api_key = "wrong-key"
            [model.z_matching]
            base_url = "https://api.example.com/v1/"
            api_key = "right-key"
        "#,
        )
        .unwrap();
        assert_eq!(
            provider_setup_api_key_from_toml(&config).as_deref(),
            Some("right-key")
        );
        let mut config = config;
        config["model"].as_table_mut().unwrap().remove("z_matching");
        assert_eq!(provider_setup_api_key_from_toml(&config), None);
    }

    #[test]
    fn setup_key_fallback_accepts_setup_binding_without_endpoint() {
        let config: toml::Value = toml::from_str(
            r#"
            [model.a_unrelated]
            api_key = "wrong-key"
            [model.z_setup]
            model_provider = "setup"
            api_key = "setup-key"
        "#,
        )
        .unwrap();
        assert_eq!(
            provider_setup_api_key_from_toml(&config).as_deref(),
            Some("setup-key")
        );
    }

    #[test]
    fn setup_key_fallback_rejects_stale_setup_model_at_other_endpoint() {
        let config: toml::Value = toml::from_str(
            r#"
            [endpoints]
            models_base_url = "https://current.example.com/v1"
            [model.old_setup]
            model_provider = "setup"
            base_url = "https://old.example.com/v1"
            api_key = "old-key"
        "#,
        )
        .unwrap();
        assert_eq!(provider_setup_api_key_from_toml(&config), None);
    }

    #[test]
    fn setup_provider_key_takes_precedence_over_model_key() {
        let config: toml::Value = toml::from_str(
            r#"
            [model_providers.setup]
            api_key = "saved-provider-key"
            [model.setup_model]
            model_provider = "setup"
            api_key = "old-model-key"
        "#,
        )
        .unwrap();
        assert_eq!(
            provider_setup_api_key_from_toml(&config).as_deref(),
            Some("saved-provider-key")
        );
    }

    #[test]
    fn model_list_is_sorted_and_deduplicated() {
        let body = json!({
            "data": [
                {"id": "z-model"},
                {"id": "a-model", "name": "A model"},
                {"id": "z-model", "name": "Duplicate"},
            ]
        });
        let models = parse_openai_models_list(&body);
        assert_eq!(
            models,
            vec![
                ProviderModel {
                    id: "a-model".to_owned(),
                    name: "A model".to_owned(),
                    context_window: None,
                    supports_thinking: false,
                },
                ProviderModel {
                    id: "z-model".to_owned(),
                    name: "z-model".to_owned(),
                    context_window: None,
                    supports_thinking: false,
                },
            ]
        );
    }

    #[test]
    fn rejects_auth_error_text_as_api_key() {
        assert!(!is_plausible_provider_api_key(
            "Authentication required — your session has expired"
        ));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert_eq!(
            persist_provider_setup_at(
                &path,
                &ProviderSetup {
                    base_url: "https://api.example.com/v1".to_owned(),
                    api_key: "Authentication required".to_owned(),
                    model_id: "test-model".to_owned(),
                    model_name: "Test model".to_owned(),
                    format: SetupWireFormat::Openai,
                    provider_id: CUSTOM_PROVIDER_ID.to_owned(),
                },
            )
            .unwrap_err(),
            ProviderSetupError::InvalidApiKey
        );
    }

    #[test]
    fn persist_writes_provider_and_model_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\ntheme = \"dark\"\n").unwrap();
        persist_provider_setup_at(
            &path,
            &ProviderSetup {
                base_url: "https://openrouter.ai/api/v1".to_owned(),
                api_key: "sk-test".to_owned(),
                model_id: "anthropic/claude-sonnet-4-6".to_owned(),
                model_name: "Claude Sonnet".to_owned(),
                format: SetupWireFormat::Openai,
                provider_id: "openrouter".to_owned(),
            },
        )
        .unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("theme = \"dark\""));
        assert!(body.contains("models_base_url = \"https://openrouter.ai/api/v1\""));
        assert!(body.contains("models_list_url = \"https://openrouter.ai/api/v1/models\""));
        assert!(body.contains("default = \"anthropic/claude-sonnet-4-6\""));
        assert!(body.contains("api_key = \"sk-test\""));
        assert!(body.contains("model_provider = \"setup\""));
        assert!(body.contains("id = \"openrouter\""));
        let parsed: toml::Value = toml::from_str(&body).unwrap();
        assert_eq!(
            provider_setup_api_key_from_toml(&parsed).as_deref(),
            Some("sk-test")
        );
    }

    #[cfg(unix)]
    #[test]
    fn persist_restricts_config_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        persist_provider_setup_at(
            &path,
            &ProviderSetup {
                base_url: "https://api.example.com/v1".to_owned(),
                api_key: "test-key".to_owned(),
                model_id: "test-model".to_owned(),
                model_name: "Test model".to_owned(),
                format: SetupWireFormat::Openai,
                provider_id: CUSTOM_PROVIDER_ID.to_owned(),
            },
        )
        .unwrap();
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "provider credentials must be owner-only");
    }
}

#[cfg(test)]
mod context_window_metadata_tests {
    use super::provider_model_context_window;
    use serde_json::json;

    #[test]
    fn reads_provider_context_metadata() {
        assert_eq!(provider_model_context_window(&json!({"id":"a","context_length":1_000_000})), Some(1_000_000));
        assert_eq!(provider_model_context_window(&json!({"id":"a","context_window":"2000000"})), Some(2_000_000));
        assert_eq!(provider_model_context_window(&json!({"id":"a","top_provider":{"context_length":400000}})), Some(400_000));
        assert_eq!(provider_model_context_window(&json!({"id":"a","inputTokenLimit":1048576})), Some(1_048_576));
        assert_eq!(provider_model_context_window(&json!({"id":"a"})), None);
        assert_eq!(provider_model_context_window(&json!({"id":"a","context_length":0})), None);
    }
}
