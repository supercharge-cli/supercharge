//! Interactive `/setup` provider configuration.
//!
//! Collects an OpenAI-compatible base URL and credential, lists models from
//! that endpoint, and writes the chosen model into `config.toml`.

use std::net::IpAddr;
use std::path::Path;

use serde_json::Value;
use thiserror::Error;

const MAX_MODELS: usize = 500;
const MAX_BASE_URL_LEN: usize = 500;
const MAX_API_KEY_LEN: usize = 8192;
const MAX_MODEL_ID_LEN: usize = 200;
pub const SETUP_PROVIDER_ID: &str = "setup";
pub const CUSTOM_PROVIDER_ID: &str = "custom";
pub const CUSTOM_MODEL_ID: &str = "__custom__";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupWireFormat {
    Openai,
    Anthropic,
}

impl Default for SetupWireFormat {
    fn default() -> Self {
        Self::Openai
    }
}

impl SetupWireFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
        }
    }

    pub fn api_backend(self) -> &'static str {
        match self {
            Self::Openai => "chat_completions",
            Self::Anthropic => "messages",
        }
    }

    pub fn auth_scheme(self) -> &'static str {
        match self {
            Self::Openai => "bearer",
            Self::Anthropic => "x_api_key",
        }
    }

    pub fn parse(raw: &str) -> Self {
        if raw.eq_ignore_ascii_case("anthropic") || raw.eq_ignore_ascii_case("messages") {
            Self::Anthropic
        } else {
            Self::Openai
        }
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
        description: "Nous Portal inference",
        base_url: "https://inference-api.nousresearch.com/v1",
        default_model: "deepseek/deepseek-v4-flash",
        format: SetupWireFormat::Openai,
        curated: &["deepseek/deepseek-v4-flash"],
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderModel {
    pub id: String,
    pub name: String,
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

fn curated_models(provider_id: Option<&str>) -> Vec<ProviderModel> {
    let Some(id) = provider_id.filter(|id| !id.is_empty() && *id != CUSTOM_PROVIDER_ID) else {
        return Vec::new();
    };
    if let Some(profile) = find_setup_provider(id) {
        return profile
            .curated
            .iter()
            .map(|model| ProviderModel {
                id: (*model).to_owned(),
                name: (*model).to_owned(),
            })
            .collect();
    }
    Vec::new()
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
            let name = obj
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(id);
            (id, name)
        };
        if id.is_empty() || id.len() > MAX_MODEL_ID_LEN || is_non_chat_model(id) {
            continue;
        }
        models.push(ProviderModel {
            id: id.to_owned(),
            name: name.to_owned(),
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
            request = request
                .header("HTTP-Referer", "https://supercharge.ai")
                .header("X-Title", "Supercharge AI");
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

/// GET `{base_url}/models`. Tries without a key, then with the key.
/// Falls back to the provider's curated list when the live catalog is empty.
pub fn fetch_provider_models(
    base_url: &str,
    api_key: &str,
    format: SetupWireFormat,
    provider_id: Option<&str>,
) -> Result<Vec<ProviderModel>, ProviderSetupError> {
    let base_url = normalize_provider_base_url(base_url)?;
    if api_key.len() > MAX_API_KEY_LEN {
        return Err(ProviderSetupError::ApiKeyTooLong);
    }
    let endpoint = provider_models_list_url(&base_url);
    let mut models = match fetch_models_once(&endpoint, &base_url, "", format) {
        Ok(models) => models,
        Err(_) => Vec::new(),
    };
    if !api_key.trim().is_empty() {
        match fetch_models_once(&endpoint, &base_url, api_key, format) {
            Ok(authed) => {
                models.extend(authed);
                models.sort_by(|a, b| a.id.cmp(&b.id));
                models.dedup_by(|a, b| a.id == b.id);
            }
            Err(error) if models.is_empty() => {
                let curated = curated_models(provider_id);
                if curated.is_empty() {
                    return Err(error);
                }
                return Ok(curated);
            }
            Err(_) => {}
        }
    }
    if models.is_empty() {
        let curated = curated_models(provider_id);
        if curated.is_empty() {
            return Err(ProviderSetupError::Fetch(
                "provider returned no models".to_owned(),
            ));
        }
        return Ok(curated);
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
    if setup.api_key.len() > MAX_API_KEY_LEN {
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
        setup.api_key.trim(),
        model_id,
        model_name,
        setup.format,
        setup.provider_id.trim(),
    );
    crate::util::config::atomic_write_string(path, &document.to_string())?;
    Ok(())
}

fn apply_provider_setup_to_document(
    document: &mut toml_edit::DocumentMut,
    base_url: &str,
    api_key: &str,
    model_id: &str,
    model_name: &str,
    format: SetupWireFormat,
    provider_id: &str,
) {
    document["endpoints"]["xai_api_base_url"] = toml_edit::value(base_url);
    document["endpoints"]["models_base_url"] = toml_edit::value(base_url);
    document["endpoints"]["models_list_url"] = toml_edit::value(provider_models_list_url(base_url));

    document["models"]["default"] = toml_edit::value(model_id);

    document["model_providers"][SETUP_PROVIDER_ID]["base_url"] = toml_edit::value(base_url);
    document["model_providers"][SETUP_PROVIDER_ID]["api_backend"] =
        toml_edit::value(format.api_backend());
    document["model_providers"][SETUP_PROVIDER_ID]["id"] =
        toml_edit::value(if provider_id.is_empty() {
            CUSTOM_PROVIDER_ID
        } else {
            provider_id
        });
    if api_key.is_empty() {
        if let Some(table) = document["model_providers"][SETUP_PROVIDER_ID].as_table_mut() {
            table.remove("api_key");
        }
    } else {
        document["model_providers"][SETUP_PROVIDER_ID]["api_key"] = toml_edit::value(api_key);
    }

    document["model"][model_id]["model"] = toml_edit::value(model_id);
    document["model"][model_id]["base_url"] = toml_edit::value(base_url);
    document["model"][model_id]["name"] = toml_edit::value(model_name);
    document["model"][model_id]["api_backend"] = toml_edit::value(format.api_backend());
    document["model"][model_id]["auth_scheme"] = toml_edit::value(format.auth_scheme());
    document["model"][model_id]["model_provider"] = toml_edit::value(SETUP_PROVIDER_ID);
    if format == SetupWireFormat::Anthropic {
        document["model"][model_id]["extra_headers"]["anthropic-version"] =
            toml_edit::value("2023-06-01");
    }
    if api_key.is_empty() {
        if let Some(table) = document["model"][model_id].as_table_mut() {
            table.remove("api_key");
        }
    } else {
        document["model"][model_id]["api_key"] = toml_edit::value(api_key);
    }
}

/// Credential written by `/setup` for custom-endpoint catalog fetches.
pub fn provider_setup_api_key_from_toml(config: &toml::Value) -> Option<String> {
    config
        .get("model_providers")
        .and_then(|providers| providers.get(SETUP_PROVIDER_ID))
        .and_then(|provider| provider.get("api_key"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            config
                .get("model")
                .and_then(toml::Value::as_table)
                .and_then(|models| {
                    models.values().find_map(|entry| {
                        entry
                            .get("api_key")
                            .and_then(toml::Value::as_str)
                            .map(str::trim)
                            .filter(|key| !key.is_empty())
                            .map(str::to_owned)
                    })
                })
        })
}

pub fn load_existing_provider_setup() -> Option<(String, String)> {
    let path = crate::util::config::user_config_path();
    let content = crate::util::config::read_to_string_or_empty(&path).ok()?;
    if content.trim().is_empty() {
        return None;
    }
    let parsed = content.parse::<toml::Value>().ok()?;
    let base_url = parsed
        .get("endpoints")
        .and_then(|endpoints| {
            endpoints
                .get("models_base_url")
                .or_else(|| endpoints.get("xai_api_base_url"))
        })
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned)?;
    let api_key = provider_setup_api_key_from_toml(&parsed).unwrap_or_default();
    Some((base_url, api_key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
                },
                ProviderModel {
                    id: "o3-mini".to_owned(),
                    name: "o3 mini".to_owned(),
                },
            ]
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
}
