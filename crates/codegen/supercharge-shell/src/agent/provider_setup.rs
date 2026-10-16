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

/// Built-in providers from the XibeCode catalog, plus a Custom row in the TUI.
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
            "openai-fast",
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
        default_model: "mistral-large-latest",
        format: SetupWireFormat::Openai,
        curated: &["mistral-large-latest", "codestral-latest"],
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
            context_window,
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

pub fn infer_thinking_from_id(id: &str) -> bool {
    let s = id.to_ascii_lowercase();
    s.contains("grok")
        || s.contains("reason")
        || s.contains("think")
        || s.contains("o1")
        || s.contains("o3")
        || s.contains("o4")
}

/// Canonical effort ids this model accepts, strongest first.
///
/// From xAI docs (`reasoning_effort` on grok-4.6 / grok-4.5 / grok-4.3):
/// - grok-4.6 and later, grok-4.20: `low` | `medium` | `high` | `xhigh`
/// - grok-4.5 / grok-4.3: `low` | `medium` | `high` (no `xhigh`; cannot disable)
/// Other thinking-capable ids get the three-level set.
pub fn reasoning_effort_ids_for_model(id: &str) -> &'static [&'static str] {
    if !infer_thinking_from_id(id) {
        return &[];
    }
    let s = id.to_ascii_lowercase();
    if s.contains("4.6")
        || s.contains("4.20")
        || s.contains("grok-5")
        || s.contains("xhigh")
        || s.contains("-max")
    {
        return &["xhigh", "high", "medium", "low"];
    }
    &["high", "medium", "low"]
}

fn model_entry_supports_thinking(entry: &Value, id: &str, name: &str) -> bool {
    if let Some(obj) = entry.as_object() {
        for key in [
            "supports_reasoning_effort",
            "supportsReasoningEffort",
            "reasoning",
            "thinking",
        ] {
            if obj.get(key).and_then(Value::as_bool) == Some(true) {
                return true;
            }
        }
        if obj
            .get("reasoning_efforts")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
            || obj
                .get("reasoningEfforts")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
        {
            return true;
        }
    }
    infer_thinking_from_id(id) || infer_thinking_from_id(name)
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
            let name = ["display_name", "name"]
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
    document_table_mut(document, "models")?.insert("default", toml_edit::value(model_id));

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
    if api_key.is_empty() {
        model.remove("api_key");
    } else {
        model.insert("api_key", toml_edit::value(api_key));
    }
    Ok(())
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
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
                supports_thinking: false,
            },
            ProviderModel {
                id: "openai".to_owned(),
                name: "openai".to_owned(),
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
    fn reasoning_effort_ids_match_xai_docs() {
        assert_eq!(
            reasoning_effort_ids_for_model("grok-4.6"),
            &["xhigh", "high", "medium", "low"]
        );
        assert_eq!(
            reasoning_effort_ids_for_model("grok-4.5"),
            &["high", "medium", "low"]
        );
        assert_eq!(
            reasoning_effort_ids_for_model("grok-4.3"),
            &["high", "medium", "low"]
        );
        assert!(reasoning_effort_ids_for_model("gpt-4o").is_empty());
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
                    supports_thinking: false,
                },
                ProviderModel {
                    id: "o3-mini".to_owned(),
                    name: "o3 mini".to_owned(),
                    supports_thinking: true,
                },
            ]
        );
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
                    supports_thinking: false,
                },
                ProviderModel {
                    id: "z-model".to_owned(),
                    name: "z-model".to_owned(),
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
