use std::{env, fs, path::Path};

use serde::{Deserialize, Serialize};
use std::time::Instant;
use thiserror::Error;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderFormat {
    #[default]
    Openai,
    Anthropic,
    Responses,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderProfile {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub format: ProviderFormat,
    pub api_key_env: String,
    pub configured: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfigRequest {
    pub provider: String,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub format: Option<ProviderFormat>,
    pub api_key_env: Option<String>,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider identifier is invalid")]
    InvalidProvider,
    #[error("provider base URL must use http or https")]
    InvalidBaseUrl,
    #[error("provider model is empty or too long")]
    InvalidModel,
    #[error("provider API-key environment variable is invalid")]
    InvalidApiKeyEnv,
    #[error("provider format is not supported by the runtime")]
    UnsupportedFormat,
    #[error("provider configuration I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("provider configuration serialization failed: {0}")]
    Toml(#[from] toml::ser::Error),
}

#[derive(Debug, Clone)]
struct BuiltinProvider {
    id: &'static str,
    name: &'static str,
    base_url: &'static str,
    default_model: &'static str,
    format: ProviderFormat,
    api_key_env: &'static str,
}

const BUILTINS: &[BuiltinProvider] = &[
    BuiltinProvider {
        id: "pollinations",
        name: "Pollinations",
        base_url: "https://gen.pollinations.ai/v1",
        default_model: "openai",
        format: ProviderFormat::Openai,
        api_key_env: "POLLINATIONS_API_KEY",
    },
    BuiltinProvider {
        id: "openai",
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        default_model: "gpt-5.4",
        format: ProviderFormat::Openai,
        api_key_env: "OPENAI_API_KEY",
    },
    BuiltinProvider {
        id: "anthropic",
        name: "Anthropic",
        base_url: "https://api.anthropic.com/v1",
        default_model: "claude-sonnet-4-6",
        format: ProviderFormat::Anthropic,
        api_key_env: "ANTHROPIC_API_KEY",
    },
    BuiltinProvider {
        id: "openrouter",
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        default_model: "anthropic/claude-sonnet-4-6",
        format: ProviderFormat::Openai,
        api_key_env: "OPENROUTER_API_KEY",
    },
    BuiltinProvider {
        id: "deepseek",
        name: "DeepSeek",
        base_url: "https://api.deepseek.com/v1",
        default_model: "deepseek-chat",
        format: ProviderFormat::Openai,
        api_key_env: "DEEPSEEK_API_KEY",
    },
    BuiltinProvider {
        id: "google",
        name: "Google AI Studio",
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        default_model: "gemini-2.5-pro",
        format: ProviderFormat::Openai,
        api_key_env: "GOOGLE_API_KEY",
    },
    BuiltinProvider {
        id: "groq",
        name: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        default_model: "llama-3.3-70b-versatile",
        format: ProviderFormat::Openai,
        api_key_env: "GROQ_API_KEY",
    },
    BuiltinProvider {
        id: "zai",
        name: "Z.AI",
        base_url: "https://api.z.ai/api/paas/v4",
        default_model: "glm-5.1",
        format: ProviderFormat::Openai,
        api_key_env: "ZAI_API_KEY",
    },
    BuiltinProvider {
        id: "kimi-coding",
        name: "Kimi Coding",
        base_url: "https://api.moonshot.ai/v1",
        default_model: "kimi-k2.6",
        format: ProviderFormat::Openai,
        api_key_env: "KIMI_API_KEY",
    },
    BuiltinProvider {
        id: "qwen",
        name: "Qwen Cloud",
        base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        default_model: "qwen3.6-plus",
        format: ProviderFormat::Openai,
        api_key_env: "DASHSCOPE_API_KEY",
    },
    BuiltinProvider {
        id: "routingrun",
        name: "Routing.run",
        base_url: "https://api.routing.run/v1",
        default_model: "route/glm-5.1",
        format: ProviderFormat::Openai,
        api_key_env: "ROUTINGRUN_API_KEY",
    },
    BuiltinProvider {
        id: "zenllm",
        name: "ZenLLM",
        base_url: "https://api.zenllm.org/v1",
        default_model: "zhipu/glm-5.1",
        format: ProviderFormat::Openai,
        api_key_env: "ZENLLM_API_KEY",
    },
    BuiltinProvider {
        id: "nous",
        name: "Nous Research",
        base_url: "https://inference-api.nousresearch.com/v1",
        default_model: "deepseek/deepseek-v4-flash",
        format: ProviderFormat::Openai,
        api_key_env: "NOUS_API_KEY",
    },
];

#[derive(Debug, Clone, Serialize)]
pub struct ProviderHealth {
    pub provider: String,
    pub base_url: String,
    pub ok: bool,
    pub status: Option<u16>,
    pub latency_ms: u128,
    pub error: Option<String>,
}

pub async fn health_check(profile: &ProviderProfile) -> ProviderHealth {
    let started = Instant::now();
    let endpoint = format!("{}/models", profile.base_url.trim_end_matches('/'));
    let result = async {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .map_err(|error| error.to_string())?;
        let mut request = client.get(endpoint);
        if let Ok(key) = std::env::var(&profile.api_key_env)
            && !key.trim().is_empty()
        {
            request = request.bearer_auth(key);
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        let status = response.status();
        Ok::<(bool, Option<u16>, Option<String>), String>((
            status.is_success() || status == reqwest::StatusCode::UNAUTHORIZED,
            Some(status.as_u16()),
            None,
        ))
    }
    .await;
    match result {
        Ok((ok, status, error)) => ProviderHealth {
            provider: profile.id.clone(),
            base_url: profile.base_url.clone(),
            ok,
            status,
            latency_ms: started.elapsed().as_millis(),
            error,
        },
        Err(error) => ProviderHealth {
            provider: profile.id.clone(),
            base_url: profile.base_url.clone(),
            ok: false,
            status: None,
            latency_ms: started.elapsed().as_millis(),
            error: Some(error),
        },
    }
}

pub fn builtin_profiles() -> Vec<ProviderProfile> {
    BUILTINS.iter().map(profile_for_builtin).collect()
}

pub fn configured_profile() -> ProviderProfile {
    let provider = env::var("SUPERAGENT_PROVIDER").unwrap_or_else(|_| "pollinations".to_owned());
    let builtin = BUILTINS
        .iter()
        .find(|entry| entry.id.eq_ignore_ascii_case(provider.trim()));
    let base_url = env::var("SUPERAGENT_PROVIDER_BASE_URL")
        .or_else(|_| env::var("SUPERAGENT_BASE_URL"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| builtin.map(|entry| entry.base_url.to_owned()))
        .unwrap_or_else(|| "https://api.openai.com/v1".to_owned());
    let model = env::var("SUPERAGENT_MODEL")
        .or_else(|_| env::var("SUPERAGENT_DEFAULT_MODEL"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| builtin.map(|entry| entry.default_model.to_owned()))
        .unwrap_or_else(|| "gpt-4o-mini".to_owned());
    let format = env::var("SUPERAGENT_PROVIDER_FORMAT")
        .ok()
        .and_then(|value| parse_format(&value))
        .or_else(|| builtin.map(|entry| entry.format.clone()))
        .unwrap_or_default();
    let api_key_env = env::var("SUPERAGENT_PROVIDER_API_KEY_ENV")
        .or_else(|_| env::var("SUPERAGENT_API_KEY_ENV"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| builtin.map(|entry| entry.api_key_env.to_owned()))
        .unwrap_or_else(|| "OPENAI_API_KEY".to_owned());
    let id = provider.trim().to_ascii_lowercase();
    let name = builtin
        .map(|entry| entry.name.to_owned())
        .unwrap_or_else(|| "Custom provider".to_owned());
    ProviderProfile {
        id,
        name,
        base_url,
        model,
        format,
        api_key_env,
        configured: false,
    }
}

pub fn validate_request(request: ProviderConfigRequest) -> Result<ProviderProfile, ProviderError> {
    let provider = request.provider.trim().to_ascii_lowercase();
    if provider.is_empty()
        || provider.len() > 64
        || !provider
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
    {
        return Err(ProviderError::InvalidProvider);
    }
    let builtin = BUILTINS.iter().find(|entry| entry.id == provider);
    let base_url = request
        .base_url
        .or_else(|| builtin.map(|entry| entry.base_url.to_owned()))
        .unwrap_or_default();
    let base_url = base_url.trim().trim_end_matches('/').to_owned();
    let parsed = reqwest::Url::parse(&base_url).map_err(|_| ProviderError::InvalidBaseUrl)?;
    let loopback = matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
    if !matches!(parsed.scheme(), "http" | "https") || (parsed.scheme() == "http" && !loopback) {
        return Err(ProviderError::InvalidBaseUrl);
    }
    let model = request
        .model
        .or_else(|| builtin.map(|entry| entry.default_model.to_owned()))
        .unwrap_or_default();
    let model = model.trim().to_owned();
    if model.is_empty() || model.len() > 200 {
        return Err(ProviderError::InvalidModel);
    }
    let format = request
        .format
        .or_else(|| builtin.map(|entry| entry.format.clone()))
        .unwrap_or_default();
    let api_key_env = request
        .api_key_env
        .or_else(|| builtin.map(|entry| entry.api_key_env.to_owned()))
        .unwrap_or_else(|| "OPENAI_API_KEY".to_owned());
    if api_key_env.is_empty()
        || api_key_env.len() > 128
        || !api_key_env
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
    {
        return Err(ProviderError::InvalidApiKeyEnv);
    }
    let name = builtin
        .map(|entry| entry.name.to_owned())
        .unwrap_or_else(|| "Custom OpenAI provider".to_owned());
    Ok(ProviderProfile {
        id: provider,
        name,
        base_url,
        model,
        format,
        api_key_env: api_key_env.clone(),
        configured: env::var(&api_key_env).is_ok(),
    })
}

pub fn write_runtime_config(
    path: impl AsRef<Path>,
    profile: &ProviderProfile,
) -> Result<(), ProviderError> {
    let mut model = toml::map::Map::new();
    model.insert(
        "model".to_owned(),
        toml::Value::String(profile.model.clone()),
    );
    model.insert("name".to_owned(), toml::Value::String(profile.name.clone()));
    model.insert(
        "description".to_owned(),
        toml::Value::String(format!("{} provider profile", profile.name)),
    );
    model.insert(
        "base_url".to_owned(),
        toml::Value::String(profile.base_url.clone()),
    );
    model.insert(
        "api_base_url".to_owned(),
        toml::Value::String(profile.base_url.clone()),
    );
    model.insert(
        "env_key".to_owned(),
        toml::Value::String(profile.api_key_env.clone()),
    );
    model.insert(
        "api_backend".to_owned(),
        toml::Value::String(profile.format.api_backend().to_owned()),
    );
    let agent_type = if crate::browser::is_e2b_runtime() {
        "supercharge-e2b"
    } else {
        "supercharge-core"
    };
    model.insert(
        "agent_type".to_owned(),
        toml::Value::String(agent_type.to_owned()),
    );
    model.insert("context_window".to_owned(), toml::Value::Integer(128_000));
    let mut model_section = toml::map::Map::new();
    model_section.insert(profile.id.clone(), toml::Value::Table(model));
    let mut document = toml::map::Map::new();
    document.insert("model".to_owned(), toml::Value::Table(model_section));
    if agent_type == "supercharge-e2b" {
        let mut agent = toml::map::Map::new();
        agent.insert("name".to_owned(), toml::Value::String(agent_type.to_owned()));
        document.insert("agent".to_owned(), toml::Value::Table(agent));
    }
    let rendered = toml::to_string_pretty(&toml::Value::Table(document))?;
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("toml.tmp");
    fs::write(&temporary, rendered)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn profile_for_builtin(entry: &BuiltinProvider) -> ProviderProfile {
    ProviderProfile {
        id: entry.id.to_owned(),
        name: entry.name.to_owned(),
        base_url: entry.base_url.to_owned(),
        model: entry.default_model.to_owned(),
        format: entry.format.clone(),
        api_key_env: entry.api_key_env.to_owned(),
        configured: env::var(entry.api_key_env).is_ok(),
    }
}

impl ProviderFormat {
    pub fn api_backend(&self) -> &'static str {
        match self {
            Self::Openai => "chat_completions",
            Self::Anthropic => "messages",
            Self::Responses => "responses",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Openai => "Chat Completions",
            Self::Anthropic => "Messages",
            Self::Responses => "Responses",
        }
    }
}

pub fn parse_format(value: &str) -> Option<ProviderFormat> {
    match value
        .trim()
        .to_ascii_lowercase()
        .replace('-', "_")
        .as_str()
    {
        "openai" | "openai_compatible" | "chat_completions" | "chat_completion"
        | "chatcompletions" => Some(ProviderFormat::Openai),
        "anthropic" | "messages" | "anthropic_messages" => Some(ProviderFormat::Anthropic),
        "responses" | "response" => Some(ProviderFormat::Responses),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{
        ProviderConfigRequest, ProviderFormat, builtin_profiles, validate_request,
        write_runtime_config,
    };

    #[test]
    fn exposes_multiple_openai_compatible_builtins() {
        let profiles = builtin_profiles();
        assert!(profiles.iter().any(|profile| profile.id == "pollinations"));
        assert!(profiles.iter().any(|profile| profile.id == "openrouter"));
        assert!(
            profiles
                .iter()
                .any(|profile| profile.id == "anthropic"
                    && profile.format == ProviderFormat::Anthropic)
        );
        assert!(
            profiles
                .iter()
                .filter(|profile| profile.id != "anthropic")
                .all(|profile| profile.format == ProviderFormat::Openai)
        );
    }

    #[test]
    fn validates_custom_loopback_openai_endpoint_without_persisting_a_secret() {
        let profile = validate_request(ProviderConfigRequest {
            provider: "local-llm".to_owned(),
            base_url: Some("http://127.0.0.1:8000/v1".to_owned()),
            model: Some("local-model".to_owned()),
            format: Some(ProviderFormat::Openai),
            api_key_env: Some("LOCAL_LLM_API_KEY".to_owned()),
        })
        .unwrap();
        let directory = tempdir().unwrap();
        let path = directory.path().join("provider.toml");
        write_runtime_config(&path, &profile).unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("[model.local-llm]"));
        assert!(text.contains("http://127.0.0.1:8000/v1"));
        assert!(!text.contains("secret"));
        assert!(text.contains("LOCAL_LLM_API_KEY"));
        assert!(text.contains("api_backend = \"chat_completions\""));
        assert!(text.contains("context_window = 128000"));
    }

    #[test]
    fn writes_responses_and_messages_backends() {
        for (format, backend) in [
            (ProviderFormat::Responses, "responses"),
            (ProviderFormat::Anthropic, "messages"),
        ] {
            let profile = validate_request(ProviderConfigRequest {
                provider: "custom".to_owned(),
                base_url: Some("https://api.example.com/v1".to_owned()),
                model: Some("m".to_owned()),
                format: Some(format),
                api_key_env: Some("CUSTOM_API_KEY".to_owned()),
            })
            .unwrap();
            let directory = tempdir().unwrap();
            let path = directory.path().join("provider.toml");
            write_runtime_config(&path, &profile).unwrap();
            let text = std::fs::read_to_string(path).unwrap();
            assert!(text.contains(&format!("api_backend = \"{backend}\"")));
        }
    }

    #[test]
    fn rejects_public_plain_http_custom_endpoints() {
        assert!(
            validate_request(ProviderConfigRequest {
                provider: "custom".to_owned(),
                base_url: Some("http://provider.example/v1".to_owned()),
                model: Some("model".to_owned()),
                format: Some(ProviderFormat::Openai),
                api_key_env: Some("CUSTOM_API_KEY".to_owned()),
            })
            .is_err()
        );
    }
}
