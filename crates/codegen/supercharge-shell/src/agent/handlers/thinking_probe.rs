//! Thinking capability probe for models whose support is unknown.
//!
//! When the effort picker opens (TUI) or an effort is set (TUI or ACP) on a model that the
//! catalog does not mark as thinking-capable, a few tiny requests find out which thinking
//! parameter the provider accepts (see `supercharge_sampler::thinking_probe`). The result is
//! cached per provider + model on disk, folded into the model catalog, and used to shape real
//! requests. The probe is bounded by [`PROBE_BUDGET`] so it never stalls the UI for long.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use agent_client_protocol as acp;
use supercharge_sampler::thinking_probe::{self, ThinkingProbeResult};

use crate::agent::handlers::model_switch::{self, ConfigNotice, SwitchEffort};
use crate::agent::mvp_agent::MvpAgent;

/// Extension method: `{ sessionId, force? }` → `{ modelId, supported, shape, efforts, cached }`.
pub(crate) const THINKING_PROBE_METHOD: &str = "supercharge/session/thinking_probe";

/// Upper bound on one probe, all requests included.
pub(crate) const PROBE_BUDGET: Duration = Duration::from_secs(10);

fn in_flight() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Outcome reported to callers.
#[derive(Debug, Clone)]
pub(crate) struct ProbeOutcome {
    pub model_id: String,
    pub result: Option<ThinkingProbeResult>,
    /// True when the answer came from the cache or the catalog without sending requests.
    pub cached: bool,
    /// The catalog already knows the model thinks (no probe needed).
    pub known_supported: bool,
}

impl ProbeOutcome {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        let supported = self.known_supported || self.result.as_ref().is_some_and(|r| r.supported());
        serde_json::json!({
            "modelId": self.model_id,
            "supported": supported,
            "known": self.known_supported || self.result.is_some(),
            "shape": self.result.as_ref().map(|r| r.shape),
            "efforts": self.result.as_ref().map(|r| r.efforts.clone()).unwrap_or_default(),
            "cached": self.cached,
        })
    }
}

/// Probe the session's current model when its thinking support is unknown.
/// `force` re-probes even when a cached result exists.
pub(crate) async fn probe_session_model(
    agent: &MvpAgent,
    session_id: &acp::SessionId,
    force: bool,
) -> Result<ProbeOutcome, acp::Error> {
    let handle = agent
        .session_handle_waiting_for_load(session_id)
        .await
        .ok_or_else(|| acp::Error::invalid_params().data("unknown session id"))?;
    let model_id = handle.model_id.clone();
    let models = agent.models_manager.models();
    let key = crate::agent::remote_config::resolve_catalog_key(&models, &model_id)
        .unwrap_or_else(|| model_id.clone());
    let Some(entry) = models.get(key.0.as_ref()).cloned() else {
        return Err(acp::Error::invalid_params().data("unknown model"));
    };
    let mut outcome = ProbeOutcome {
        model_id: key.0.to_string(),
        result: None,
        cached: true,
        known_supported: false,
    };
    // Explicit config wins; Messages-API models carry thinking natively.
    let explicit = agent.cfg.borrow().config_models.get(key.0.as_ref()).is_some_and(|o| {
        o.thinking.is_some() || o.supports_reasoning_effort.is_some() || !o.reasoning_efforts.is_empty()
    });
    if entry.info.api_backend == supercharge_sampling_types::ApiBackend::Messages || explicit {
        outcome.known_supported = entry.info.supports_reasoning_effort;
        return Ok(outcome);
    }
    if !force {
        if let Some(hit) = thinking_probe::lookup(&entry.info.base_url, &entry.info.model) {
            outcome.result = Some(hit);
            return Ok(outcome);
        }
        if entry.info.supports_reasoning_effort {
            outcome.known_supported = true;
            return Ok(outcome);
        }
    }

    let flight_key = thinking_probe::cache_key(&entry.info.base_url, &entry.info.model);
    if !in_flight().lock().unwrap_or_else(|e| e.into_inner()).insert(flight_key.clone()) {
        return Ok(outcome);
    }
    let result = run_probe(agent, key.0.as_ref(), &models).await;
    in_flight().lock().unwrap_or_else(|e| e.into_inner()).remove(&flight_key);

    outcome.cached = false;
    outcome.result = result;
    if outcome.result.is_some() {
        // Fold the cached result into the catalog and push refreshed options to the client.
        let cfg = agent.cfg.borrow().clone();
        agent.models_manager.apply_config(cfg);
        let _ = model_switch::apply(
            agent,
            acp::SetSessionModelRequest::new(session_id.clone(), model_id.clone()),
            SwitchEffort::Preserve,
            ConfigNotice::Send,
        )
        .await;
    }
    Ok(outcome)
}

async fn run_probe(
    agent: &MvpAgent,
    model_key: &str,
    models: &indexmap::IndexMap<String, crate::agent::config::ModelEntry>,
) -> Option<ThinkingProbeResult> {
    let session_key = agent
        .auth_manager
        .current_or_expired()
        .map(|a| a.key.clone());
    let disable_api_key_auth = agent.cfg.borrow().grok_com_config.api_key_auth_disabled();
    let endpoints = agent.models_manager.endpoints();
    let cfg = crate::agent::config::resolve_aux_model_sampling_config(
        model_key,
        models,
        &endpoints,
        session_key.as_deref(),
        disable_api_key_auth,
        None,
        None,
    )?;
    let model = cfg.model.clone();
    let client = supercharge_sampler::SamplingClient::new(cfg).ok()?;
    tokio::time::timeout(PROBE_BUDGET + Duration::from_secs(1), client.probe_thinking(&model, PROBE_BUDGET))
        .await
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use crate::agent::config::{ConfigModelOverride, EndpointsConfig};

    #[test]
    fn thinking_override_forces_support_on_and_off() {
        let endpoints = EndpointsConfig::default();
        let on = ConfigModelOverride { thinking: Some(true), ..Default::default() };
        let entry = on.apply("gpt-4o", None, &endpoints);
        assert!(entry.info.supports_reasoning_effort);
        assert!(!entry.info.reasoning_efforts.is_empty());

        let off = ConfigModelOverride { thinking: Some(false), ..Default::default() };
        let entry = off.apply("o3", Some(on.apply("o3", None, &endpoints)), &endpoints);
        assert!(!entry.info.supports_reasoning_effort);
        assert!(entry.info.reasoning_efforts.is_empty());
    }
}
