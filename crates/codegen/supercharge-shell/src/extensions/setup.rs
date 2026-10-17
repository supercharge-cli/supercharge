//! Provider setup over ACP (`_supercharge/setup/*`), the same logic as the TUI `/setup`.
//!
//! | Method | Params | Result |
//! |---|---|---|
//! | `setup/status` | `{}` | provider config state + built-in providers |
//! | `setup/models` | `{providerId?, baseUrl?, apiKey?, format?}` | `{models}` |
//! | `setup/save` | `{providerId?, baseUrl?, apiKey?, modelId, modelName?, format?}` | `{saved, modelId, models}` |
//! | `setup/device_start` | `{}` | `{userCode, verificationUri, verificationUriComplete, expiresIn, interval}` |
//! | `setup/device_poll` | `{}` | `{status: pending\|slow_down\|done, interval}` |
//! | `setup/device_cancel` | `{}` | `{cancelled}` |
//! | `setup/image_status` | `{}` | image provider state |
//! | `setup/image_save` | `{providerId?, baseUrl?, apiKey?, modelId, format?}` | `{saved, modelId}` |

use std::sync::Mutex;

use agent_client_protocol as acp;
use serde_json::{Value, json};

use crate::agent::mvp_agent::MvpAgent;
use crate::agent::nous_oauth::{self, DeviceCodePending, DevicePollStatus};
use crate::agent::provider_setup::{
    self as ps, ProviderModel, ProviderSetup, ProviderSetupError, SetupWireFormat,
};
use crate::extensions::{ExtResult, to_raw_response};

pub(crate) const STATUS: &str = "supercharge/setup/status";
pub(crate) const MODELS: &str = "supercharge/setup/models";
pub(crate) const SAVE: &str = "supercharge/setup/save";
pub(crate) const DEVICE_START: &str = "supercharge/setup/device_start";
pub(crate) const DEVICE_POLL: &str = "supercharge/setup/device_poll";
pub(crate) const DEVICE_CANCEL: &str = "supercharge/setup/device_cancel";
pub(crate) const IMAGE_STATUS: &str = "supercharge/setup/image_status";
pub(crate) const IMAGE_SAVE: &str = "supercharge/setup/image_save";

/// All setup methods (without the leading underscore), for routing and capability lists.
pub(crate) const METHODS: [&str; 8] = [
    STATUS, MODELS, SAVE, DEVICE_START, DEVICE_POLL, DEVICE_CANCEL, IMAGE_STATUS, IMAGE_SAVE,
];

/// Provider id that signs in with the device-code flow instead of an API key.
const OAUTH_PROVIDER: &str = "nous";

fn pending_device() -> &'static Mutex<Option<DeviceCodePending>> {
    static PENDING: Mutex<Option<DeviceCodePending>> = Mutex::new(None);
    &PENDING
}

fn invalid(msg: impl Into<String>) -> acp::Error {
    acp::Error::invalid_params().data(msg.into())
}

fn internal(msg: impl Into<String>) -> acp::Error {
    acp::Error::internal_error().data(msg.into())
}

fn setup_err(e: ProviderSetupError) -> acp::Error {
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

/// Resolved connection target for `models` / `save` requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub base_url: String,
    pub api_key: String,
    pub format: SetupWireFormat,
    pub provider_id: String,
    pub default_model: Option<String>,
}

/// Resolve `{providerId?, baseUrl?, apiKey?, format?}` the way `/setup` does: a built-in
/// provider id supplies base URL, format and default model; an omitted API key falls back to
/// the saved key when the saved provider matches.
pub(crate) fn resolve_target(
    p: &Value,
    saved: Option<&ps::ExistingProviderSetup>,
) -> Result<Target, acp::Error> {
    let provider_id = str_param(p, &["providerId", "provider_id", "provider"]).unwrap_or("");
    let profile = (!provider_id.is_empty())
        .then(|| ps::find_setup_provider(provider_id))
        .flatten();
    if !provider_id.is_empty() && profile.is_none() && str_param(p, &["baseUrl", "base_url"]).is_none() {
        return Err(invalid(format!("unknown provider `{provider_id}`")));
    }
    let base_url = str_param(p, &["baseUrl", "base_url"])
        .map(str::to_owned)
        .or_else(|| profile.map(|pr| pr.base_url.to_owned()))
        .or_else(|| saved.map(|s| s.base_url.clone()))
        .ok_or_else(|| invalid("providerId or baseUrl is required"))?;
    let base_url = ps::normalize_provider_base_url(&base_url).map_err(setup_err)?;
    let format = str_param(p, &["format", "apiBackend", "api_backend"])
        .map(SetupWireFormat::parse)
        .or_else(|| profile.map(|pr| pr.format))
        .or_else(|| saved.filter(|s| s.base_url == base_url).map(|s| s.format))
        .unwrap_or_default();
    let api_key = match str_param(p, &["apiKey", "api_key"]) {
        Some(k) => k.to_owned(),
        None => saved
            .filter(|s| {
                ps::normalize_provider_base_url(&s.base_url).ok().as_deref() == Some(base_url.as_str())
            })
            .map(|s| s.api_key.clone())
            .unwrap_or_default(),
    };
    let provider_id = profile
        .map(|pr| pr.id.to_owned())
        .unwrap_or_else(|| provider_id.to_owned());
    Ok(Target {
        base_url,
        api_key,
        format,
        provider_id,
        default_model: profile.map(|pr| pr.default_model.to_owned()),
    })
}

fn model_json(m: &ProviderModel) -> Value {
    json!({
        "id": m.id,
        "name": m.name,
        "supportsThinking": m.supports_thinking,
        "contextWindow": m.context_window,
    })
}

fn providers_json() -> Value {
    Value::Array(
        ps::setup_provider_profiles()
            .iter()
            .map(|p| {
                json!({
                    "id": p.id,
                    "name": p.name,
                    "description": p.description,
                    "baseUrl": p.base_url,
                    "defaultModel": p.default_model,
                    "format": p.format.as_str(),
                    "curated": p.curated,
                    "auth": if p.id == OAUTH_PROVIDER { "oauth_device" } else { "api_key" },
                })
            })
            .collect(),
    )
}

fn saved_default_model() -> Option<String> {
    let path = crate::util::config::user_config_path();
    let content = crate::util::config::read_to_string_or_empty(&path).ok()?;
    let parsed = toml::from_str::<toml::Value>(&content).ok()?;
    parsed
        .get("model")
        .and_then(|m| m.as_str().map(str::to_owned).or_else(|| m.get("default")?.as_str().map(str::to_owned)))
}

/// `setup/status` result.
pub(crate) fn status_json() -> Value {
    let saved = ps::load_existing_provider_setup_details();
    json!({
        "configured": saved.is_some(),
        "providerId": saved.as_ref().map(|s| s.provider_id.clone()),
        "baseUrl": saved.as_ref().map(|s| s.base_url.clone()),
        "format": saved.as_ref().map(|s| s.format.as_str()),
        "hasApiKey": saved.as_ref().is_some_and(|s| !s.api_key.is_empty()),
        "modelId": saved_default_model(),
        "oauth": { "nous": { "signedIn": nous_oauth::signed_in() } },
        "deviceLoginPending": pending_device().lock().unwrap_or_else(|e| e.into_inner()).is_some(),
        "configPath": crate::util::config::user_config_path().to_string_lossy(),
        "providers": providers_json(),
    })
}

async fn fetch_models(target: &Target) -> Result<Vec<ProviderModel>, acp::Error> {
    let t = target.clone();
    tokio::task::spawn_blocking(move || {
        ps::fetch_provider_models(
            &t.base_url,
            &t.api_key,
            t.format,
            (!t.provider_id.is_empty()).then_some(t.provider_id.as_str()),
        )
    })
    .await
    .map_err(|e| internal(e.to_string()))?
    .map_err(setup_err)
}

fn params_value(args: &acp::ExtRequest) -> Result<Value, acp::Error> {
    let raw = args.params.get();
    if raw.trim().is_empty() || raw.trim() == "null" {
        return Ok(json!({}));
    }
    serde_json::from_str(raw).map_err(|e| invalid(e.to_string()))
}

/// Route one `supercharge/setup/*` request (method name without the leading underscore).
pub(crate) async fn handle(agent: &MvpAgent, method: &str, args: &acp::ExtRequest) -> ExtResult {
    let p = params_value(args)?;
    match method {
        STATUS => to_raw_response(&status_json()),
        MODELS => {
            let saved = ps::load_existing_provider_setup_details();
            let target = resolve_target(&p, saved.as_ref())?;
            let models = fetch_models(&target).await?;
            to_raw_response(&json!({
                "models": models.iter().map(model_json).collect::<Vec<_>>(),
                "providerId": target.provider_id,
                "baseUrl": target.base_url,
                "format": target.format.as_str(),
                "defaultModel": target.default_model,
            }))
        }
        SAVE | IMAGE_SAVE => {
            let saved = ps::load_existing_provider_setup_details();
            let target = resolve_target(&p, saved.as_ref())?;
            let requested = str_param(&p, &["modelId", "model_id", "model"]).map(str::to_owned);
            // Like `/setup`: list models to validate the credential and pick a default.
            let listed = match fetch_models(&target).await {
                Ok(models) => models,
                Err(e) if requested.is_some() && method == IMAGE_SAVE => {
                    tracing::debug!(error = ?e, "image setup: model list unavailable");
                    Vec::new()
                }
                Err(e) => return Err(e),
            };
            let model_id = requested
                .or_else(|| target.default_model.clone().filter(|d| listed.iter().any(|m| &m.id == d)))
                .or_else(|| listed.first().map(|m| m.id.clone()))
                .ok_or_else(|| invalid("provider returned no models; pass modelId"))?;
            let model_name = str_param(&p, &["modelName", "model_name"])
                .map(str::to_owned)
                .or_else(|| listed.iter().find(|m| m.id == model_id).map(|m| m.name.clone()))
                .unwrap_or_else(|| model_id.clone());
            let setup = ProviderSetup {
                base_url: target.base_url.clone(),
                api_key: target.api_key.clone(),
                model_id: model_id.clone(),
                model_name,
                format: target.format,
                provider_id: target.provider_id.clone(),
            };
            if method == IMAGE_SAVE {
                let s = setup.clone();
                tokio::task::spawn_blocking(move || crate::agent::image_provider::persist_image_provider_setup(&s))
                    .await
                    .map_err(|e| internal(e.to_string()))?
                    .map_err(setup_err)?;
                return to_raw_response(&json!({ "saved": true, "modelId": model_id }));
            }
            let s = setup.clone();
            tokio::task::spawn_blocking(move || ps::persist_provider_setup(&s))
                .await
                .map_err(|e| internal(e.to_string()))?
                .map_err(setup_err)?;
            // Same as the TUI: hot-reload the catalog so new sessions see the provider.
            let reloaded = crate::extensions::session_admin::handle_reload_models(agent).is_ok();
            to_raw_response(&json!({
                "saved": true,
                "modelId": model_id,
                "reloaded": reloaded,
                "models": listed.iter().map(model_json).collect::<Vec<_>>(),
                "message": format!(
                    "Saved provider settings. Default model: `{model_id}`. Start a new Supercharge session to use it."
                ),
            }))
        }
        DEVICE_START => {
            let pending = tokio::task::spawn_blocking(nous_oauth::request_device_code)
                .await
                .map_err(|e| internal(e.to_string()))?
                .map_err(internal)?;
            let out = json!({
                "userCode": pending.user_code,
                "verificationUri": pending.verification_uri,
                "verificationUriComplete": pending.verification_uri_complete,
                "expiresIn": pending.expires_in,
                "interval": pending.interval,
            });
            *pending_device().lock().unwrap_or_else(|e| e.into_inner()) = Some(pending);
            to_raw_response(&out)
        }
        DEVICE_POLL => {
            let pending = pending_device()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .ok_or_else(|| invalid("no device login in progress; call setup/device_start"))?;
            let polled = tokio::task::spawn_blocking({
                let pending = pending.clone();
                move || nous_oauth::poll_once(&pending)
            })
            .await
            .map_err(|e| internal(e.to_string()))?;
            match polled {
                Ok(status) => {
                    let mut interval = pending.interval.max(1);
                    let name = match status {
                        DevicePollStatus::Pending => "pending",
                        DevicePollStatus::SlowDown => {
                            interval += 5;
                            if let Some(p) = pending_device().lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                                p.interval = interval;
                            }
                            "slow_down"
                        }
                        DevicePollStatus::Done => {
                            *pending_device().lock().unwrap_or_else(|e| e.into_inner()) = None;
                            "done"
                        }
                    };
                    to_raw_response(&json!({ "status": name, "interval": interval }))
                }
                Err(message) => {
                    *pending_device().lock().unwrap_or_else(|e| e.into_inner()) = None;
                    Err(invalid(message))
                }
            }
        }
        DEVICE_CANCEL => {
            let had = pending_device().lock().unwrap_or_else(|e| e.into_inner()).take().is_some();
            to_raw_response(&json!({ "cancelled": had }))
        }
        IMAGE_STATUS => {
            let saved = crate::agent::image_provider::load_existing_image_provider_setup();
            to_raw_response(&json!({
                "configured": saved.is_some(),
                "baseUrl": saved.as_ref().map(|s| s.0.clone()),
                "hasApiKey": saved.as_ref().is_some_and(|s| !s.1.is_empty()),
                "modelId": saved.as_ref().map(|s| s.2.clone()).filter(|m| !m.is_empty()),
                "providers": providers_json(),
            }))
        }
        other => Err(acp::Error::method_not_found().data(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> ps::ExistingProviderSetup {
        ps::ExistingProviderSetup {
            base_url: "https://api.example.test/v1".into(),
            api_key: "sk-saved".into(),
            format: SetupWireFormat::Responses,
            provider_id: "custom".into(),
        }
    }

    #[test]
    fn builtin_provider_fills_url_format_and_default_model() {
        let p = ps::setup_provider_profiles()[0];
        let t = resolve_target(&json!({"providerId": p.id, "apiKey": "k"}), None).unwrap();
        assert_eq!(t.provider_id, p.id);
        assert_eq!(t.format, p.format);
        assert_eq!(t.api_key, "k");
        assert_eq!(t.default_model.as_deref(), Some(p.default_model));
    }

    #[test]
    fn omitted_key_reuses_saved_key_only_for_same_base_url() {
        let s = saved();
        let t = resolve_target(&json!({"baseUrl": "https://api.example.test/v1"}), Some(&s)).unwrap();
        assert_eq!(t.api_key, "sk-saved");
        assert_eq!(t.format, SetupWireFormat::Responses);
        let t = resolve_target(&json!({"baseUrl": "https://other.test/v1"}), Some(&s)).unwrap();
        assert_eq!(t.api_key, "");
        let t = resolve_target(&json!({}), Some(&s)).unwrap();
        assert_eq!(t.base_url, "https://api.example.test/v1");
    }

    #[test]
    fn unknown_provider_without_url_is_rejected() {
        assert!(resolve_target(&json!({"providerId": "nope-provider"}), None).is_err());
        assert!(resolve_target(&json!({}), None).is_err());
    }

    #[test]
    fn status_lists_providers_with_auth_kind() {
        let v = status_json();
        let providers = v["providers"].as_array().unwrap();
        assert!(!providers.is_empty());
        assert!(providers.iter().all(|p| p["auth"].is_string() && p["baseUrl"].is_string()));
        assert!(v.get("configured").is_some());
    }
}
