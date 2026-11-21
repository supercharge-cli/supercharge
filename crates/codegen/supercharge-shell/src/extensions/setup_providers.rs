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
//! | `setup/provider_add_custom` | `{name, baseUrl, format?, apiKey?, id?, defaultModel?, validate?}` | `{provider: ProviderInfo, models: [Model]}` |
//! | `setup/provider_update_custom` | `{providerId, name?, baseUrl?, format?, apiKey?, defaultModel?, validate?}` | `{provider: ProviderInfo, models: [Model], active, reloaded}` |
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
pub(crate) const PROVIDER_UPDATE_CUSTOM: &str = "supercharge/setup/provider_update_custom";
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
    let auth_login = if custom {
        Value::Null
    } else {
        auth::view_for(id)
    };
    let sign_in_pending = auth_login.get("status").and_then(Value::as_str) == Some("pending");
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
        "authLogin": auth_login,
        "signInPending": sign_in_pending,
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
        c.default_model.as_deref(),
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
    // Re-adding an existing custom provider at the same URL keeps its key and default model.
    let existing = config
        .and_then(|c| ps::saved_custom_provider(c, &id))
        .filter(|c| c.base_url == base_url);
    let api_key = match str_param(p, &["apiKey", "api_key"]) {
        Some(k) => k.to_owned(),
        None => existing
            .as_ref()
            .map(|c| c.api_key.clone())
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
        default_model: existing.and_then(|c| c.default_model),
    })
}

/// `apiKey` as sent: `Some("")` clears the key, `None` leaves it unchanged.
fn raw_key_param(p: &Value) -> Option<&str> {
    ["apiKey", "api_key"]
        .iter()
        .find_map(|k| p.get(*k).and_then(Value::as_str))
        .map(str::trim)
}

/// Apply `provider_update_custom` fields to a saved custom provider. Fields left out keep
/// their saved value. Moving to another origin needs an explicit `apiKey` (or `""`), so a
/// saved key is never sent to a host it was not entered for.
pub(crate) fn custom_update_from_params(
    p: &Value,
    config: Option<&toml::Value>,
) -> Result<SavedCustomProvider, acp::Error> {
    // `providerId` like every other per-provider call; `id` too, matching provider_add_custom.
    let id = str_param(p, &["providerId", "provider_id", "provider", "id"])
        .ok_or_else(|| invalid("providerId is required"))?;
    let mut custom = config
        .and_then(|c| ps::saved_custom_provider(c, id))
        .ok_or_else(|| invalid(format!("unknown custom provider `{id}`")))?;
    if let Some(name) = str_param(p, &["name", "displayName"]) {
        custom.name = ps::validate_custom_provider_name(name).map_err(setup_err)?;
    }
    if let Some(raw) = str_param(p, &["format", "apiBackend", "api_backend"]) {
        custom.format = parse_format(Some(raw))?;
    }
    let key = raw_key_param(p);
    if let Some(raw) = str_param(p, &["baseUrl", "base_url"]) {
        let base_url = ps::normalize_provider_base_url(raw).map_err(setup_err)?;
        if !same_origin(&base_url, &custom.base_url) && key.is_none() && !custom.api_key.is_empty()
        {
            return Err(invalid(
                "baseUrl moves to another host: pass apiKey for it (or \"\" to clear the saved key)",
            ));
        }
        if base_url != custom.base_url {
            // A model id chosen for the old endpoint may not exist on the new one.
            custom.default_model = None;
        }
        custom.base_url = base_url;
    }
    if let Some(key) = key {
        if !ps::is_plausible_provider_api_key(key) {
            return Err(setup_err(ProviderSetupError::InvalidApiKey));
        }
        custom.api_key = key.to_owned();
    }
    Ok(custom)
}

fn same_origin(a: &str, b: &str) -> bool {
    match (url::Url::parse(a), url::Url::parse(b)) {
        (Ok(a), Ok(b)) => a.origin() == b.origin(),
        _ => false,
    }
}

/// Pick the custom provider's default model: an explicit `defaultModel` (which must be listed
/// when models were fetched), else the saved one if still listed, else the first listed model.
pub(crate) fn choose_custom_default(
    explicit: Option<&str>,
    saved: Option<&str>,
    listed: &[ProviderModel],
) -> Result<Option<String>, acp::Error> {
    let is_listed = |id: &str| listed.iter().any(|m| m.id == id);
    if let Some(model) = explicit {
        if !listed.is_empty() && !is_listed(model) {
            return Err(invalid(format!(
                "defaultModel `{model}` is not in the provider's model list"
            )));
        }
        return Ok(Some(model.to_owned()));
    }
    if let Some(model) = saved.filter(|m| listed.is_empty() || is_listed(m)) {
        return Ok(Some(model.to_owned()));
    }
    Ok(listed.first().map(|m| m.id.clone()))
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
        PROVIDER_UPDATE_CUSTOM => update_custom(agent, &path, p).await,
        PROVIDER_SET_KEY => set_key(agent, &path, p).await,
        PROVIDER_REMOVE => remove(&path, p),
        _ => return None,
    })
}

fn default_model_param(p: &Value) -> Option<&str> {
    str_param(p, &["defaultModel", "default_model", "modelId", "model_id"])
}

/// Validate (optionally), choose the default model and save a custom provider.
async fn save_custom(
    path: &Path,
    p: &Value,
    mut custom: SavedCustomProvider,
) -> Result<(SavedCustomProvider, Vec<ProviderModel>), acp::Error> {
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
    custom.default_model = choose_custom_default(
        default_model_param(p),
        custom.default_model.as_deref(),
        &models,
    )?;
    ps::persist_custom_provider_at(path, &custom).map_err(setup_err)?;
    Ok((custom, models))
}

async fn add_custom(path: &Path, p: &Value) -> Result<Value, acp::Error> {
    let custom = custom_from_params(p, read_config(path).as_ref())?;
    let (custom, models) = save_custom(path, p, custom).await?;
    let config = read_config(path);
    Ok(json!({
        "provider": custom_info(&custom, config.as_ref()),
        "models": models.iter().map(model_json).collect::<Vec<_>>(),
    }))
}

async fn update_custom(
    agent: &crate::agent::mvp_agent::MvpAgent,
    path: &Path,
    p: &Value,
) -> Result<Value, acp::Error> {
    let before = read_config(path);
    let custom = custom_update_from_params(p, before.as_ref())?;
    let (custom, models) = save_custom(path, p, custom).await?;
    let active = active_id(before.as_ref()).as_deref() == Some(custom.id.as_str());
    let mut reloaded = false;
    if active {
        let target = KeyTarget::Custom(custom.clone());
        reloaded = rewrite_active(
            agent,
            path,
            before.as_ref(),
            &target,
            &custom.api_key,
            &models,
        )?;
    }
    let config = read_config(path);
    Ok(json!({
        "provider": custom_info(&custom, config.as_ref()),
        "models": models.iter().map(model_json).collect::<Vec<_>>(),
        "active": active,
        "reloaded": reloaded,
    }))
}

/// The active provider also carries its URL and key in `[model_providers.setup]` and the
/// default model entry; rewrite them through the `/setup` writer and reload the catalog.
fn rewrite_active(
    agent: &crate::agent::mvp_agent::MvpAgent,
    path: &Path,
    config: Option<&toml::Value>,
    target: &KeyTarget,
    api_key: &str,
    models: &[ProviderModel],
) -> Result<bool, acp::Error> {
    let custom_default = match target {
        KeyTarget::Custom(c) => c.default_model.clone(),
        KeyTarget::Builtin { .. } => None,
    };
    let model_id = config
        .and_then(default_model_from)
        .filter(|m| models.is_empty() || models.iter().any(|l| &l.id == m))
        .or(custom_default)
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
    Ok(crate::extensions::session_admin::handle_reload_models(agent).is_ok())
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
    let active = active_id(config.as_ref()).as_deref() == Some(target.id());
    let reloaded = if active {
        rewrite_active(agent, path, config.as_ref(), &target, api_key, &models)?
    } else {
        false
    };
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

    fn model(id: &str) -> ProviderModel {
        ProviderModel {
            id: id.into(),
            name: id.into(),
            supports_thinking: false,
            context_window: None,
        }
    }

    #[test]
    fn custom_default_model_is_a_listed_model_when_models_are_known() {
        let listed = [model("m-a"), model("m-b")];
        assert_eq!(
            choose_custom_default(None, None, &listed)
                .unwrap()
                .as_deref(),
            Some("m-a")
        );
        assert_eq!(
            choose_custom_default(Some("m-b"), None, &listed)
                .unwrap()
                .as_deref(),
            Some("m-b")
        );
        assert!(choose_custom_default(Some("m-x"), None, &listed).is_err());
        // A saved default survives while still listed, else the first listed model wins.
        assert_eq!(
            choose_custom_default(None, Some("m-b"), &listed)
                .unwrap()
                .as_deref(),
            Some("m-b")
        );
        assert_eq!(
            choose_custom_default(None, Some("gone"), &listed)
                .unwrap()
                .as_deref(),
            Some("m-a")
        );
        // `validate: false`: only an explicit or saved default; otherwise null.
        assert_eq!(
            choose_custom_default(Some("typed"), None, &[])
                .unwrap()
                .as_deref(),
            Some("typed")
        );
        assert_eq!(choose_custom_default(None, None, &[]).unwrap(), None);
    }

    #[test]
    fn custom_default_model_is_reported_and_used_by_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut c = custom_from_params(
            &json!({"name": "Gw", "baseUrl": "https://gw.example.test/v1", "apiKey": "sk-gw"}),
            None,
        )
        .unwrap();
        c.default_model = choose_custom_default(None, None, &[model("m-a")]).unwrap();
        ps::persist_custom_provider_at(&path, &c).unwrap();
        let config = read(&path);
        let info = provider_info(Some(&config), "custom-gw").unwrap();
        assert_eq!(info["defaultModel"], "m-a");
        let t = crate::extensions::setup::resolve_target_in(
            &json!({"providerId": "custom-gw"}),
            None,
            Some(&config),
        )
        .unwrap();
        assert_eq!(t.default_model.as_deref(), Some("m-a"));
        // Re-adding at the same URL keeps the default.
        let again = custom_from_params(
            &json!({"name": "Gw", "id": "custom-gw", "baseUrl": "https://gw.example.test/v1"}),
            Some(&config),
        )
        .unwrap();
        assert_eq!(again.default_model.as_deref(), Some("m-a"));
    }

    #[test]
    fn update_custom_patches_fields_and_guards_the_key_on_host_change() {
        let config = cfg(
            "[provider_credentials.custom-gw]\ncustom = true\nname = \"Gw\"\nbase_url = \"https://gw.example.test/v1\"\nformat = \"openai\"\napi_key = \"sk-gw-secret\"\ndefault_model = \"m-a\"\n",
        );
        // Rename only: everything else is kept.
        let c = custom_update_from_params(
            &json!({"id": "custom-gw", "name": "Gateway"}),
            Some(&config),
        )
        .unwrap();
        assert_eq!(c.name, "Gateway");
        assert_eq!(c.api_key, "sk-gw-secret");
        assert_eq!(c.default_model.as_deref(), Some("m-a"));
        // Same origin, new path: key kept, default reset for the new endpoint.
        let c = custom_update_from_params(
            &json!({"providerId": "custom-gw", "baseUrl": "https://gw.example.test/v2", "format": "responses"}),
            Some(&config),
        )
        .unwrap();
        assert_eq!(c.base_url, "https://gw.example.test/v2");
        assert_eq!(c.format, SetupWireFormat::Responses);
        assert_eq!(c.api_key, "sk-gw-secret");
        assert_eq!(c.default_model, None);
        // Another host without apiKey is refused, so the key never reaches it.
        let err = custom_update_from_params(
            &json!({"providerId": "custom-gw", "baseUrl": "https://other.example.test/v1"}),
            Some(&config),
        )
        .unwrap_err();
        assert!(!format!("{err:?}").contains("sk-gw-secret"));
        // ... unless a key (or "" to clear) is given.
        let c = custom_update_from_params(
            &json!({"providerId": "custom-gw", "baseUrl": "https://other.example.test/v1", "apiKey": ""}),
            Some(&config),
        )
        .unwrap();
        assert_eq!(c.api_key, "");
        let c = custom_update_from_params(
            &json!({"providerId": "custom-gw", "baseUrl": "https://other.example.test/v1", "apiKey": "sk-new"}),
            Some(&config),
        )
        .unwrap();
        assert_eq!(c.api_key, "sk-new");
        // Unknown ids, built-ins and bad fields are rejected.
        for bad in [
            json!({"providerId": "custom-nope", "name": "x"}),
            json!({"providerId": "openai", "name": "x"}),
            json!({"name": "x"}),
            json!({"providerId": "custom-gw", "format": "soap"}),
            json!({"providerId": "custom-gw", "baseUrl": "http://remote.example.test/v1"}),
            json!({"providerId": "custom-gw", "apiKey": "Authentication required. Run /login"}),
        ] {
            assert!(
                custom_update_from_params(&bad, Some(&config)).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    #[serial_test::serial(setup_auth)]
    fn provider_info_shows_an_in_progress_sign_in() {
        let idle = provider_info(None, "nous").unwrap();
        assert_eq!(idle["authLogin"], Value::Null);
        assert_eq!(idle["signInPending"], false);
        let (tx, rx) = std::sync::mpsc::channel::<auth::Finish>();
        let flow = auth::Flow {
            started: auth::Started {
                url: "https://portal.example.test/device".into(),
                user_code: Some("ABCD".into()),
                expires_in: 600,
                interval: 5,
            },
            wait: Box::new(move |_cancel, _interval| {
                rx.recv().unwrap_or(auth::Finish::Error("gone".into()))
            }),
            on_cancel: None,
        };
        let started = auth::install(
            "nous",
            auth::AuthKind::Device,
            flow,
            std::sync::Arc::new(|_| {}),
        );
        let info = provider_info(None, "nous").unwrap();
        assert_eq!(info["signInPending"], true);
        assert_eq!(info["authLogin"]["loginId"], started["loginId"]);
        assert_eq!(info["authLogin"]["status"], "pending");
        assert!(info["authLogin"].get("userCode").is_none());
        let list = list_json(None, Path::new("/tmp/config.toml"));
        let device_provider = list["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "mistral-auth")
            .cloned()
            .unwrap();
        assert_eq!(device_provider["signInPending"], false);
        let _ = tx.send(auth::Finish::Done);
        auth::cancel_current();
    }
}
