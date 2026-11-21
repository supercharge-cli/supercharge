//! Provider catalogue and credentials over ACP (`_supercharge/setup/provider*`).
//!
//! Studio lists, adds, authenticates and selects providers through these methods so it
//! never re-implements provider logic. Every method reuses the `/setup` code paths in
//! [`crate::agent::provider_setup`]: the same URL normalisation, key checks, model listing
//! and `config.toml` writer. No API key, token or secret is ever returned.
//!
//! | Method | Params | Result |
//! |---|---|---|
//! | `setup/providers_list` | `{}` | `{activeProviderId, activeModelId, configPath, providers: [ProviderInfo]}` |
//! | `setup/provider_status` | `{providerId}` | `ProviderInfo` |
//! | `setup/provider_add_custom` | `{name, baseUrl, format?, apiKey?, id?, validate?}` | `{provider: ProviderInfo, models: [Model]}` |
//! | `setup/provider_set_key` | `{providerId, apiKey, validate?}` | `{saved, providerId, active, reloaded, models: [Model], provider: ProviderInfo}` |
//! | `setup/provider_remove` | `{providerId}` | `{removed, providerId}` |

use std::path::Path;

use agent_client_protocol as acp;
use serde_json::{Value, json};

use crate::agent::provider_setup::{
    self as ps, ProviderModel, ProviderSetup, ProviderSetupError, SavedCustomProvider,
    SetupProviderProfile, SetupWireFormat,
};
use crate::extensions::setup_auth as auth;

pub(crate) const PROVIDERS_LIST: &str = "supercharge/setup/providers_list";
pub(crate) const PROVIDER_STATUS: &str = "supercharge/setup/provider_status";
pub(crate) const PROVIDER_ADD_CUSTOM: &str = "supercharge/setup/provider_add_custom";
pub(crate) const PROVIDER_SET_KEY: &str = "supercharge/setup/provider_set_key";
pub(crate) const PROVIDER_REMOVE: &str = "supercharge/setup/provider_remove";

fn invalid(msg: impl Into<String>) -> acp::Error {
    acp::Error::invalid_params().data(msg.into())
}

fn internal(msg: impl Into<String>) -> acp::Error {
    acp::Error::internal_error().data(msg.into())
}

pub(crate) fn setup_err(e: ProviderSetupError) -> acp::Error {
    match e {
        ProviderSetupError::Io(m) => internal(m),
        other => invalid(other.to_string()),
    }
}

fn str_param<'a>(p: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| p.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn provider_id_param(p: &Value) -> Result<&str, acp::Error> {
    str_param(p, &["providerId", "provider_id", "provider"])
        .ok_or_else(|| invalid("providerId is required"))
}

/// Strict wire-format parsing for user input: unknown names are rejected, not defaulted.
pub(crate) fn parse_format(raw: Option<&str>) -> Result<SetupWireFormat, acp::Error> {
    match raw.map(str::to_ascii_lowercase).as_deref() {
        None | Some("openai" | "chat_completions" | "chat-completions") => {
            Ok(SetupWireFormat::Openai)
        }
        Some("anthropic" | "messages") => Ok(SetupWireFormat::Anthropic),
        Some("responses") => Ok(SetupWireFormat::Responses),
        Some(other) => Err(invalid(format!(
            "unknown format `{other}`: use openai, anthropic or responses"
        ))),
    }
}

pub(crate) fn model_json(m: &ProviderModel) -> Value {
    json!({
        "id": m.id,
        "name": m.name,
        "supportsThinking": m.supports_thinking,
        "contextWindow": m.context_window,
    })
}

/// Built-in providers Studio may offer (the picker list; hidden ids are excluded).
fn listed_builtin(provider_id: &str) -> Option<&'static SetupProviderProfile> {
    ps::setup_provider_profiles().find(|p| p.id == provider_id)
}

fn active_id(config: Option<&toml::Value>) -> Option<String> {
    config.and_then(ps::active_setup_provider_id)
}

fn active_key_present(config: Option<&toml::Value>, provider_id: &str) -> bool {
    active_id(config).as_deref() == Some(provider_id)
        && config
            .and_then(ps::provider_setup_api_key_from_toml)
            .is_some()
}

fn info(
    id: &str,
    name: &str,
    description: &str,
    base_url: &str,
    default_model: Option<&str>,
    format: SetupWireFormat,
    curated: &[&str],
    custom: bool,
    config: Option<&toml::Value>,
) -> Value {
    let (auth_methods, key_optional, signed_in) = if custom {
        let key_optional = ps::base_url_key_optional(base_url);
        let mut methods = Vec::new();
        if key_optional {
            methods.push("none");
        }
        methods.push("api_key");
        (methods, key_optional, None)
    } else {
        (
            auth::auth_methods(id),
            auth::key_optional(id),
            auth::signed_in(id, config),
        )
    };
    let has_api_key = auth_methods.contains(&"api_key")
        && (config
            .and_then(|c| ps::stored_provider_api_key(c, id))
            .is_some()
            || active_key_present(config, id));
    let active = active_id(config).as_deref() == Some(id);
    let ready = key_optional || has_api_key || signed_in == Some(true);
    json!({
        "id": id,
        "name": name,
        "description": description,
        "baseUrl": base_url,
        "defaultModel": default_model,
        "format": format.as_str(),
        "formatName": format.display_name(),
        "curated": curated,
        "custom": custom,
        "auth": if id == "nous" { "oauth_device" } else { "api_key" },
        "authMethods": auth_methods,
        "keyOptional": key_optional,
        "signedIn": signed_in,
        "hasApiKey": has_api_key,
        "active": active,
        "ready": ready,
        "modelsSource": "live",
        "modelsUrl": ps::resolve_provider_models_list_url(base_url, (!custom).then_some(id)),
    })
}

fn builtin_info(p: &SetupProviderProfile, config: Option<&toml::Value>) -> Value {
    info(
        p.id,
        p.name,
        p.description,
        p.base_url,
        Some(p.default_model),
        p.format,
        p.curated,
        false,
        config,
    )
}

fn custom_info(c: &SavedCustomProvider, config: Option<&toml::Value>) -> Value {
    info(
        &c.id,
        &c.name,
        "Custom provider",
        &c.base_url,
        None,
        c.format,
        &[],
        true,
        config,
    )
}

/// `ProviderInfo` for one built-in or custom provider id.
pub(crate) fn provider_info(config: Option<&toml::Value>, provider_id: &str) -> Option<Value> {
    if let Some(p) = listed_builtin(provider_id) {
        return Some(builtin_info(p, config));
    }
    let custom = ps::saved_custom_provider(config?, provider_id)?;
    Some(custom_info(&custom, config))
}

/// `providers_list` result: every built-in provider, then saved custom providers.
pub(crate) fn list_json(config: Option<&toml::Value>, config_path: &Path) -> Value {
    let mut providers: Vec<Value> = ps::setup_provider_profiles()
        .map(|p| builtin_info(p, config))
        .collect();
    if let Some(c) = config {
        providers.extend(
            ps::saved_custom_providers(c)
                .iter()
                .map(|p| custom_info(p, config)),
        );
    }
    json!({
        "activeProviderId": active_id(config),
        "activeModelId": config.and_then(default_model_from),
        "configPath": config_path.to_string_lossy(),
        "providers": providers,
    })
}

pub(crate) fn default_model_from(config: &toml::Value) -> Option<String> {
    config
        .get("models")
        .and_then(|m| m.get("default"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
}

/// Validated `provider_add_custom` input, before the optional model check.
pub(crate) fn custom_from_params(
    p: &Value,
    config: Option<&toml::Value>,
) -> Result<SavedCustomProvider, acp::Error> {
    let name = str_param(p, &["name", "displayName"]).ok_or_else(|| invalid("name is required"))?;
    let name = ps::validate_custom_provider_name(name).map_err(setup_err)?;
    let id = str_param(p, &["id", "providerId", "provider_id"])
        .map(str::to_owned)
        .unwrap_or_else(|| ps::custom_provider_id_from_name(&name));
    ps::validate_custom_provider_id(&id).map_err(setup_err)?;
    if config
        .and_then(|c| c.get(ps::PROVIDER_CREDENTIALS_TABLE)?.get(&id))
        .is_some_and(|slot| slot.get("custom").and_then(toml::Value::as_bool) != Some(true))
    {
        return Err(invalid(format!(
            "provider id `{id}` is already used by a saved credential"
        )));
    }
    let base_url =
        str_param(p, &["baseUrl", "base_url"]).ok_or_else(|| invalid("baseUrl is required"))?;
    let base_url = ps::normalize_provider_base_url(base_url).map_err(setup_err)?;
    let format = parse_format(str_param(p, &["format", "apiBackend", "api_backend"]))?;
    // Updating an existing custom provider without `apiKey` keeps its saved key.
    let api_key = match str_param(p, &["apiKey", "api_key"]) {
        Some(k) => k.to_owned(),
        None => config
            .and_then(|c| ps::saved_custom_provider(c, &id))
            .filter(|c| c.base_url == base_url)
            .map(|c| c.api_key)
            .unwrap_or_default(),
    };
    if !ps::is_plausible_provider_api_key(&api_key) {
        return Err(setup_err(ProviderSetupError::InvalidApiKey));
    }
    Ok(SavedCustomProvider {
        id,
        name,
        base_url,
        format,
        api_key,
    })
}

/// Where `provider_set_key` stores a key, and the endpoint used to validate it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyTarget {
    Builtin {
        id: String,
        base_url: String,
        format: SetupWireFormat,
    },
    Custom(SavedCustomProvider),
}

impl KeyTarget {
    pub(crate) fn id(&self) -> &str {
        match self {
            KeyTarget::Builtin { id, .. } => id,
            KeyTarget::Custom(c) => &c.id,
        }
    }
    pub(crate) fn base_url(&self) -> &str {
        match self {
            KeyTarget::Builtin { base_url, .. } => base_url,
            KeyTarget::Custom(c) => &c.base_url,
        }
    }
    pub(crate) fn format(&self) -> SetupWireFormat {
        match self {
            KeyTarget::Builtin { format, .. } => *format,
            KeyTarget::Custom(c) => c.format,
        }
    }
}

pub(crate) fn key_target(
    config: Option<&toml::Value>,
    provider_id: &str,
) -> Result<KeyTarget, acp::Error> {
    if let Some(p) = listed_builtin(provider_id) {
        if !auth::auth_methods(p.id).contains(&"api_key") {
            return Err(invalid(format!(
                "provider `{provider_id}` takes no API key; sign in with setup/auth_start"
            )));
        }
        return Ok(KeyTarget::Builtin {
            id: p.id.to_owned(),
            base_url: p.base_url.to_owned(),
            format: p.format,
        });
    }
    config
        .and_then(|c| ps::saved_custom_provider(c, provider_id))
        .map(KeyTarget::Custom)
        .ok_or_else(|| invalid(format!("unknown provider `{provider_id}`")))
}

/// Persist a key for a provider in its own slot (built-in) or definition (custom).
pub(crate) fn persist_key_at(
    path: &Path,
    target: &KeyTarget,
    api_key: &str,
) -> Result<(), acp::Error> {
    match target {
        KeyTarget::Builtin { id, base_url, .. } => {
            ps::persist_provider_key_at(path, id, base_url, api_key)
        }
        KeyTarget::Custom(c) => ps::persist_custom_provider_at(
            path,
            &SavedCustomProvider {
                api_key: api_key.to_owned(),
                ..c.clone()
            },
        ),
    }
    .map_err(setup_err)
}

/// What `provider_remove` deletes for a provider id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Removal {
    /// The device sign-in token file.
    DeviceSignIn,
    /// The `[provider_credentials.<id>]` slot (custom definition or stored key).
    Slot,
}

pub(crate) fn removal_for(
    config: Option<&toml::Value>,
    provider_id: &str,
) -> Result<Removal, acp::Error> {
    if active_id(config).as_deref() == Some(provider_id) {
        return Err(invalid(format!(
            "provider `{provider_id}` is the active provider; switch with setup/save first"
        )));
    }
    if provider_id == "nous" {
        return Ok(Removal::DeviceSignIn);
    }
    let known_slot = config
        .and_then(|c| c.get(ps::PROVIDER_CREDENTIALS_TABLE)?.get(provider_id))
        .is_some();
    if listed_builtin(provider_id).is_some() || known_slot {
        return Ok(Removal::Slot);
    }
    Err(invalid(format!("unknown provider `{provider_id}`")))
}

fn read_config(path: &Path) -> Option<toml::Value> {
    let content = crate::util::config::read_to_string_or_empty(path).ok()?;
    toml::from_str::<toml::Value>(&content).ok()
}

async fn fetch(
    base_url: String,
    api_key: String,
    format: SetupWireFormat,
    provider_id: Option<String>,
) -> Result<Vec<ProviderModel>, acp::Error> {
    tokio::task::spawn_blocking(move || {
        ps::fetch_provider_models(&base_url, &api_key, format, provider_id.as_deref())
    })
    .await
    .map_err(|e| internal(e.to_string()))?
    .map_err(setup_err)
}

fn validate_flag(p: &Value) -> bool {
    p.get("validate").and_then(Value::as_bool).unwrap_or(true)
}

/// Route one provider catalogue request. `None` when `method` is not one of ours.
pub(crate) async fn handle(
    agent: &crate::agent::mvp_agent::MvpAgent,
    method: &str,
    p: &Value,
) -> Option<Result<Value, acp::Error>> {
    let path = crate::util::config::user_config_path();
    Some(match method {
        PROVIDERS_LIST => Ok(list_json(read_config(&path).as_ref(), &path)),
        PROVIDER_STATUS => provider_id_param(p).and_then(|id| {
            provider_info(read_config(&path).as_ref(), id)
                .ok_or_else(|| invalid(format!("unknown provider `{id}`")))
        }),
        PROVIDER_ADD_CUSTOM => add_custom(&path, p).await,
        PROVIDER_SET_KEY => set_key(agent, &path, p).await,
        PROVIDER_REMOVE => remove(&path, p),
        _ => return None,
    })
}

async fn add_custom(path: &Path, p: &Value) -> Result<Value, acp::Error> {
    let custom = custom_from_params(p, read_config(path).as_ref())?;
    let models = if validate_flag(p) {
        fetch(
            custom.base_url.clone(),
            custom.api_key.clone(),
            custom.format,
            None,
        )
        .await?
    } else {
        Vec::new()
    };
    ps::persist_custom_provider_at(path, &custom).map_err(setup_err)?;
    let config = read_config(path);
    Ok(json!({
        "provider": custom_info(&custom, config.as_ref()),
        "models": models.iter().map(model_json).collect::<Vec<_>>(),
    }))
}

async fn set_key(
    agent: &crate::agent::mvp_agent::MvpAgent,
    path: &Path,
    p: &Value,
) -> Result<Value, acp::Error> {
    let provider_id = provider_id_param(p)?;
    let api_key =
        str_param(p, &["apiKey", "api_key"]).ok_or_else(|| invalid("apiKey is required"))?;
    let config = read_config(path);
    let target = key_target(config.as_ref(), provider_id)?;
    let builtin_id = matches!(target, KeyTarget::Builtin { .. }).then(|| target.id().to_owned());
    let models = if validate_flag(p) {
        fetch(
            target.base_url().to_owned(),
            api_key.to_owned(),
            target.format(),
            builtin_id,
        )
        .await?
    } else {
        Vec::new()
    };
    persist_key_at(path, &target, api_key)?;
    // The active provider also carries its key in `[model_providers.setup]` and the
    // default model entry; rewrite them through the `/setup` writer so the new key applies.
    let active = active_id(config.as_ref()).as_deref() == Some(target.id());
    let mut reloaded = false;
    if active {
        let model_id = config
            .as_ref()
            .and_then(default_model_from)
            .or_else(|| listed_builtin(target.id()).map(|pr| pr.default_model.to_owned()))
            .or_else(|| models.first().map(|m| m.id.clone()))
            .ok_or_else(|| invalid("no default model saved; call setup/save"))?;
        let model_name = models
            .iter()
            .find(|m| m.id == model_id)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| model_id.clone());
        let setup = ProviderSetup {
            base_url: target.base_url().to_owned(),
            api_key: api_key.to_owned(),
            model_id,
            model_name,
            format: target.format(),
            provider_id: target.id().to_owned(),
        };
        ps::persist_provider_setup_at(path, &setup).map_err(setup_err)?;
        reloaded = crate::extensions::session_admin::handle_reload_models(agent).is_ok();
    }
    let config = read_config(path);
    Ok(json!({
        "saved": true,
        "providerId": target.id(),
        "active": active,
        "reloaded": reloaded,
        "models": models.iter().map(model_json).collect::<Vec<_>>(),
        "provider": provider_info(config.as_ref(), target.id()),
    }))
}

fn remove(path: &Path, p: &Value) -> Result<Value, acp::Error> {
    let provider_id = provider_id_param(p)?;
    let removed = match removal_for(read_config(path).as_ref(), provider_id)? {
        Removal::DeviceSignIn => crate::agent::nous_oauth::sign_out(),
        Removal::Slot => {
            ps::remove_provider_credentials_at(path, provider_id).map_err(setup_err)?
        }
    };
    Ok(json!({ "removed": removed, "providerId": provider_id }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(text: &str) -> toml::Value {
        toml::from_str(text).unwrap()
    }

    fn read(path: &Path) -> toml::Value {
        read_config(path).unwrap()
    }

    #[test]
    fn list_covers_every_builtin_with_auth_and_no_secrets() {
        let config = cfg(
            "[provider_credentials.openai]\napi_key = \"sk-slot-secret\"\nbase_url = \"https://api.openai.com/v1\"\n",
        );
        let v = list_json(Some(&config), Path::new("/tmp/config.toml"));
        let providers = v["providers"].as_array().unwrap();
        assert_eq!(providers.len(), ps::setup_provider_profiles().count());
        let find = |id: &str| providers.iter().find(|p| p["id"] == id).cloned().unwrap();
        for id in [
            "openrouter",
            "agentrouter",
            "routingrun",
            "mistral",
            "mistral-auth",
            "nous",
            "openscale",
            "wiro",
            "free-ai",
        ] {
            let p = find(id);
            assert!(
                p["authMethods"].as_array().is_some_and(|m| !m.is_empty()),
                "{id}"
            );
            assert_eq!(p["custom"], false);
            assert_eq!(p["modelsSource"], "live");
            assert!(
                p["modelsUrl"]
                    .as_str()
                    .is_some_and(|u| u.starts_with("http")),
                "{id}"
            );
        }
        assert!(
            providers.iter().all(|p| p["id"] != "codex"),
            "hidden provider listed"
        );
        let openai = find("openai");
        assert_eq!(openai["hasApiKey"], true);
        assert_eq!(openai["ready"], true);
        assert_eq!(find("groq")["ready"], false);
        assert_eq!(find("ollama")["ready"], true);
        assert_eq!(find("nous")["hasApiKey"], false);
        assert_eq!(find("nous")["authMethods"], json!(["device"]));
        assert_eq!(find("mistral-auth")["authMethods"], json!(["browser"]));
        assert_eq!(find("anthropic")["formatName"], "Messages");
        assert!(!v.to_string().contains("sk-slot-secret"));
    }

    #[test]
    fn custom_params_validate_and_default_the_id() {
        let c = custom_from_params(
            &json!({"name": "My Gateway", "baseUrl": "gw.example.test/v1/models", "format": "responses", "apiKey": "sk-1"}),
            None,
        )
        .unwrap();
        assert_eq!(c.id, "custom-my-gateway");
        assert_eq!(c.base_url, "https://gw.example.test/v1");
        assert_eq!(c.format, SetupWireFormat::Responses);
        for bad in [
            json!({"baseUrl": "https://x.test"}),
            json!({"name": "x"}),
            json!({"name": "x", "baseUrl": "http://remote.example.test/v1"}),
            json!({"name": "x", "baseUrl": "ftp://x.test"}),
            json!({"name": "x", "baseUrl": "https://x.test", "format": "grpc"}),
            json!({"name": "x", "baseUrl": "https://x.test", "id": "openai"}),
            json!({"name": "x", "baseUrl": "https://x.test", "id": "Bad Id"}),
            json!({"name": "x", "baseUrl": "https://x.test", "id": "setup"}),
            json!({"name": "x", "baseUrl": "https://x.test", "apiKey": "Authentication required. Run /login"}),
        ] {
            assert!(custom_from_params(&bad, None).is_err(), "{bad}");
        }
        // A plain-http loopback server is allowed and key-optional.
        let local = custom_from_params(
            &json!({"name": "Local", "baseUrl": "http://localhost:8080/v1"}),
            None,
        )
        .unwrap();
        assert!(ps::base_url_key_optional(&local.base_url));
    }

    #[test]
    fn custom_provider_round_trips_and_resolves_like_a_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[models]\ndefault = \"gpt-5.4\"\n").unwrap();
        let c = custom_from_params(
            &json!({"name": "Gateway", "baseUrl": "https://gw.example.test/v1", "apiKey": "sk-gw"}),
            None,
        )
        .unwrap();
        ps::persist_custom_provider_at(&path, &c).unwrap();
        let config = read(&path);
        // Unrelated config survives.
        assert_eq!(default_model_from(&config).as_deref(), Some("gpt-5.4"));
        let v = list_json(Some(&config), &path);
        let row = v["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "custom-gateway")
            .cloned()
            .unwrap();
        assert_eq!(row["custom"], true);
        assert_eq!(row["hasApiKey"], true);
        assert_eq!(row["authMethods"], json!(["api_key"]));
        assert_eq!(row["modelsUrl"], "https://gw.example.test/v1/models");
        assert!(!v.to_string().contains("sk-gw"));
        // setup/models and setup/save resolve the custom id with its saved key and URL.
        let t = crate::extensions::setup::resolve_target_in(
            &json!({"providerId": "custom-gateway"}),
            None,
            Some(&config),
        )
        .unwrap();
        assert_eq!(t.base_url, "https://gw.example.test/v1");
        assert_eq!(t.api_key, "sk-gw");
        assert_eq!(t.provider_id, "custom-gateway");
        // Updating without apiKey keeps the saved key.
        let again = custom_from_params(&json!({"name": "Gateway 2", "id": "custom-gateway", "baseUrl": "https://gw.example.test/v1"}), Some(&config)).unwrap();
        assert_eq!(again.api_key, "sk-gw");
        // Remove drops it.
        assert_eq!(
            removal_for(Some(&config), "custom-gateway").unwrap(),
            Removal::Slot
        );
        assert!(ps::remove_provider_credentials_at(&path, "custom-gateway").unwrap());
        assert!(provider_info(Some(&read(&path)), "custom-gateway").is_none());
        assert!(!ps::remove_provider_credentials_at(&path, "custom-gateway").unwrap());
    }

    #[test]
    fn set_key_stores_per_provider_and_never_crosses_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let target = key_target(None, "groq").unwrap();
        persist_key_at(&path, &target, "gsk-groq").unwrap();
        let config = read(&path);
        assert_eq!(
            provider_info(Some(&config), "groq").unwrap()["hasApiKey"],
            true
        );
        assert_eq!(
            provider_info(Some(&config), "openai").unwrap()["hasApiKey"],
            false
        );
        // setup/save { providerId } then uses that provider's own key, and only for it.
        let t = crate::extensions::setup::resolve_target_in(
            &json!({"providerId": "groq"}),
            None,
            Some(&config),
        )
        .unwrap();
        assert_eq!(t.api_key, "gsk-groq");
        let t = crate::extensions::setup::resolve_target_in(
            &json!({"providerId": "openai"}),
            None,
            Some(&config),
        )
        .unwrap();
        assert_eq!(t.api_key, "");
        // Sign-in-only providers and unknown ids are rejected.
        assert!(key_target(None, "nous").is_err());
        assert!(key_target(None, "mistral-auth").is_err());
        assert!(key_target(None, "codex").is_err());
        assert!(key_target(None, "nope").is_err());
        // Error text is rejected as a key.
        assert!(persist_key_at(&path, &target, "Your session has expired").is_err());
        // Clearing a built-in key.
        assert_eq!(removal_for(Some(&config), "groq").unwrap(), Removal::Slot);
        assert!(ps::remove_provider_credentials_at(&path, "groq").unwrap());
        assert_eq!(
            provider_info(Some(&read(&path)), "groq").unwrap()["hasApiKey"],
            false
        );
    }

    #[test]
    fn remove_refuses_the_active_provider_and_unknown_ids() {
        let config = cfg(
            "[model_providers.setup]\nid = \"openai\"\nbase_url = \"https://api.openai.com/v1\"\napi_key = \"sk-a\"\n",
        );
        assert!(removal_for(Some(&config), "openai").is_err());
        assert!(removal_for(Some(&config), "nope").is_err());
        assert_eq!(
            removal_for(Some(&config), "nous").unwrap(),
            Removal::DeviceSignIn
        );
        let info = provider_info(Some(&config), "openai").unwrap();
        assert_eq!(info["active"], true);
        assert_eq!(info["hasApiKey"], true);
        assert!(!info.to_string().contains("sk-a"));
    }

    #[test]
    fn format_parsing_is_strict() {
        assert_eq!(parse_format(None).unwrap(), SetupWireFormat::Openai);
        assert_eq!(
            parse_format(Some("Messages")).unwrap(),
            SetupWireFormat::Anthropic
        );
        assert_eq!(
            parse_format(Some("responses")).unwrap(),
            SetupWireFormat::Responses
        );
        assert!(parse_format(Some("soap")).is_err());
    }
}
