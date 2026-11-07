//! Read-only account and budget text for the selected Mistral setup entry.
//!
//! Browser sign-in and a pasted API key share the Chat Completions host, but
//! they are different credentials. This module queries the public billing
//! routes on `https://api.mistral.ai/v1` and whoami on the console. It shows
//! a field only when that response includes it, and it never reads the other
//! entry's key or a process environment variable.

use crate::agent::mistral_browser_sign_in::{pinned_join, AUTH_API_BASE_URL, INFERENCE_BASE_URL};
use crate::agent::provider_setup::{active_setup_provider_id, isolated_provider_api_key};

pub const DASHBOARD_URL: &str = "https://console.mistral.ai/";

const MISTRAL_API_KEY_ID: &str = "mistral";
const MISTRAL_AUTH_ID: &str = "mistral-auth";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreditsReply {
    pub status: u16,
    pub body: String,
}

#[derive(Debug)]
pub enum CreditsTransportError {
    Network,
    Timeout,
}

pub trait CreditsTransport {
    fn get(&self, url: &str, bearer: &str) -> Result<CreditsReply, CreditsTransportError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreditsSelection {
    pub provider_id: String,
    pub label: &'static str,
    pub api_key: String,
    /// Budget object used for this mode. The other object is a separate pool.
    budget_field: &'static str,
}

pub fn selection_from_config(config: &toml::Value) -> Result<CreditsSelection, String> {
    let Some(provider_id) = active_setup_provider_id(config) else {
        return Err(other_provider(None));
    };
    let (label, budget_field) = match provider_id.as_str() {
        MISTRAL_AUTH_ID => ("Mistral Auth", "vibe_budget"),
        MISTRAL_API_KEY_ID => ("Mistral API Key", "api_budget"),
        other => return Err(other_provider(Some(other))),
    };
    let Some(api_key) = isolated_provider_api_key(config, &provider_id) else {
        return Err(format!(
            "Balance unavailable: {label} is selected, but no credential is saved for it.\nAccount dashboard: {DASHBOARD_URL}"
        ));
    };
    Ok(CreditsSelection {
        provider_id,
        label,
        api_key,
        budget_field,
    })
}

fn other_provider(id: Option<&str>) -> String {
    match id {
        Some(id) => format!(
            "Balance for `{id}` is unchanged. /credits reads Mistral Auth and Mistral API Key only. Other providers keep /usage."
        ),
        None => "Balance unavailable: no provider is selected. /usage still shows this session's local usage.".to_owned(),
    }
}

pub fn render_credits<T: CreditsTransport>(
    selection: &CreditsSelection,
    transport: &T,
) -> String {
    let whoami_url = match pinned_join(AUTH_API_BASE_URL, "vibe/whoami") {
        Ok(url) => url,
        Err(_) => {
            return unavailable("the account URL could not be built");
        }
    };
    let subscription_url = match pinned_join(INFERENCE_BASE_URL, "billing/subscription") {
        Ok(url) => url,
        Err(_) => return unavailable("the billing URL could not be built"),
    };
    let budget_url = match pinned_join(INFERENCE_BASE_URL, "billing/budget") {
        Ok(url) => url,
        Err(_) => return unavailable("the budget URL could not be built"),
    };

    let mut lines = vec![selection.label.to_owned()];
    match transport.get(&whoami_url, &selection.api_key) {
        Ok(reply) => lines.extend(render_whoami(&reply)),
        Err(CreditsTransportError::Timeout) => {
            lines.push("Plan unavailable: the account request timed out.".into());
        }
        Err(CreditsTransportError::Network) => {
            lines.push("Plan unavailable: the account request failed.".into());
        }
    }
    match transport.get(&subscription_url, &selection.api_key) {
        Ok(reply) => lines.extend(render_subscription(&reply)),
        Err(CreditsTransportError::Timeout) => {
            lines.push("Subscription unavailable: the request timed out.".into());
        }
        Err(CreditsTransportError::Network) => {
            lines.push("Subscription unavailable: the request failed.".into());
        }
    }
    match transport.get(&budget_url, &selection.api_key) {
        Ok(reply) => lines.extend(render_budget(&reply, selection.budget_field)),
        Err(CreditsTransportError::Timeout) => {
            lines.push(unavailable("the budget request timed out"));
        }
        Err(CreditsTransportError::Network) => {
            lines.push(unavailable("the budget request failed"));
        }
    }
    lines.join("\n")
}

fn render_whoami(reply: &CreditsReply) -> Vec<String> {
    if reply.status == 401 || reply.status == 403 {
        return vec![format!(
            "Plan unavailable: the account endpoint rejected this credential (HTTP {}).",
            reply.status
        )];
    }
    if reply.status == 429 {
        return vec!["Plan unavailable: the account endpoint was rate limited (HTTP 429).".into()];
    }
    if !(200..300).contains(&reply.status) {
        return vec![format!(
            "Plan unavailable: the account endpoint returned HTTP {}.",
            reply.status
        )];
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&reply.body) else {
        return vec!["Plan unavailable: the account response was not JSON.".into()];
    };
    let mut lines = Vec::new();
    if let Some(plan) = string_field(&value, &["plan_name", "user_plan.name", "name"]) {
        let kind = string_field(&value, &["plan_type", "user_plan.type", "type"]);
        match kind {
            Some(kind) => lines.push(format!("Plan: {plan} ({kind})")),
            None => lines.push(format!("Plan: {plan}")),
        }
    }
    if let Some(workspace) = string_field(
        &value,
        &["workspace", "workspace_name", "organization", "org_name"],
    ) {
        lines.push(format!("Workspace: {workspace}"));
    }
    if lines.is_empty() {
        lines.push("Plan unavailable: the account response had no plan or workspace fields.".into());
    }
    lines
}

fn render_subscription(reply: &CreditsReply) -> Vec<String> {
    if reply.status == 404 {
        return vec![
            "Subscription unavailable: this API did not return billing/subscription.".into(),
        ];
    }
    if reply.status == 401 || reply.status == 403 {
        return vec![format!(
            "Subscription unavailable: rejected (HTTP {}).",
            reply.status
        )];
    }
    if reply.status == 429 {
        return vec!["Subscription unavailable: rate limited (HTTP 429).".into()];
    }
    if !(200..300).contains(&reply.status) {
        return vec![format!(
            "Subscription unavailable: HTTP {}.",
            reply.status
        )];
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&reply.body) else {
        return vec!["Subscription unavailable: the response was not JSON.".into()];
    };
    let mut lines = Vec::new();
    if let Some(name) = value.get("plan_name").and_then(|v| v.as_str()) {
        if !name.trim().is_empty() {
            lines.push(format!("Subscription plan: {name}"));
        }
    }
    if let Some(payg) = value.get("vibe_payg_enabled").and_then(|v| v.as_bool()) {
        lines.push(format!(
            "Pay as you go: {}",
            if payg { "on" } else { "off" }
        ));
    }
    lines
}

fn render_budget(reply: &CreditsReply, field: &str) -> Vec<String> {
    if reply.status == 401 {
        return vec![unavailable(
            "the budget endpoint rejected this credential (HTTP 401)",
        )];
    }
    if reply.status == 403 {
        return vec![unavailable(
            "this credential cannot read usage (HTTP 403). The provider did not return a balance",
        )];
    }
    if reply.status == 404 {
        return vec![unavailable(
            "this API has no billing/budget endpoint (HTTP 404)",
        )];
    }
    if reply.status == 429 {
        return vec![unavailable(
            "the budget endpoint was rate limited (HTTP 429)",
        )];
    }
    if !(200..300).contains(&reply.status) {
        return vec![unavailable(&format!(
            "the budget endpoint returned HTTP {}",
            reply.status
        ))];
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&reply.body) else {
        return vec![unavailable("the budget response was not JSON")];
    };
    let Some(detail) = value.get(field).filter(|v| !v.is_null()) else {
        let other = if field == "vibe_budget" {
            "api_budget"
        } else {
            "vibe_budget"
        };
        let other_note = if value.get(other).filter(|v| !v.is_null()).is_some() {
            format!(" A separate `{other}` object was present and was not used.")
        } else {
            String::new()
        };
        return vec![unavailable(&format!(
            "the response did not include `{field}`.{other_note}"
        ))];
    };
    let mut lines = vec![format!(
        "Budget ({})",
        if field == "vibe_budget" {
            "browser sign-in pool"
        } else {
            "API key pool"
        }
    )];
    if let Some(used) = detail.get("usage_percentage").and_then(|v| v.as_f64()) {
        lines.push(format!("Used: {used}%"));
    }
    if let Some(limit) = detail.get("initial_budget").and_then(|v| v.as_f64()) {
        match detail.get("currency").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
            Some(currency) => lines.push(format!("Limit: {limit} {currency}")),
            None => lines.push(format!("Limit: {limit} (the response did not name a currency)")),
        }
    }
    if let Some(reset) = detail.get("reset_at").and_then(|v| v.as_str()) {
        if !reset.trim().is_empty() {
            lines.push(format!("Reset: {reset}"));
        }
    }
    if let Some(payg) = detail.get("payg_enabled").and_then(|v| v.as_bool()) {
        lines.push(format!(
            "Budget pay as you go: {}",
            if payg { "on" } else { "off" }
        ));
    }
    if lines.len() == 1 {
        lines.push(unavailable(
            "the budget object had no used, limit, or reset fields",
        ));
    }
    lines
}

fn unavailable(why: &str) -> String {
    format!("Balance unavailable: {why}.\nAccount dashboard: {DASHBOARD_URL}")
}

fn string_field(value: &serde_json::Value, paths: &[&str]) -> Option<String> {
    for path in paths {
        let mut current = value;
        let mut ok = true;
        for part in path.split('.') {
            match current.get(part) {
                Some(next) => current = next,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && let Some(text) = current.as_str() {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_owned());
            }
        }
    }
    None
}

pub fn credits_report_from_config(config: &toml::Value) -> String {
    let selection = match selection_from_config(config) {
        Ok(selection) => selection,
        Err(message) => return message,
    };
    let transport = match ProductionCredits::new() {
        Ok(transport) => transport,
        Err(message) => return message,
    };
    render_credits(&selection, &transport)
}

/// Read-only billing GETs. Shorter than browser sign-in so a stuck balance read fails fast.
const CREDITS_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
const CREDITS_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

struct ProductionCredits {
    client: reqwest::blocking::Client,
}

impl ProductionCredits {
    fn new() -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(CREDITS_REQUEST_TIMEOUT)
            .connect_timeout(CREDITS_CONNECT_TIMEOUT)
            .build()
            .map_err(|_| "Balance unavailable: the HTTP client could not be created.".to_owned())?;
        Ok(Self { client })
    }
}

impl CreditsTransport for ProductionCredits {
    fn get(&self, url: &str, bearer: &str) -> Result<CreditsReply, CreditsTransportError> {
        match self.client.get(url).bearer_auth(bearer).send() {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().unwrap_or_default();
                Ok(CreditsReply { status, body })
            }
            Err(error) if error.is_timeout() => Err(CreditsTransportError::Timeout),
            Err(_) => Err(CreditsTransportError::Network),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    #[test]
    fn credits_read_timeouts_are_three_seconds() {
        assert_eq!(CREDITS_REQUEST_TIMEOUT, Duration::from_secs(3));
        assert_eq!(CREDITS_CONNECT_TIMEOUT, Duration::from_secs(3));
    }

    struct MapTransport {
        calls: Mutex<Vec<(String, String)>>,
        replies: Mutex<Vec<Result<CreditsReply, CreditsTransportError>>>,
    }

    impl CreditsTransport for MapTransport {
        fn get(&self, url: &str, bearer: &str) -> Result<CreditsReply, CreditsTransportError> {
            self.calls
                .lock()
                .unwrap()
                .push((url.to_owned(), bearer.to_owned()));
            self.replies.lock().unwrap().remove(0)
        }
    }

    fn config_with(active: &str, auth_key: &str, api_key: &str) -> toml::Value {
        toml::from_str(&format!(
            r#"
[model_providers.setup]
id = "{active}"
api_key = "active-slot-should-not-override"

[provider_credentials.mistral-auth]
api_key = "{auth_key}"
mode = "browser"

[provider_credentials.mistral]
api_key = "{api_key}"
mode = "api_key"
"#
        ))
        .unwrap()
    }

    fn ok(body: serde_json::Value) -> Result<CreditsReply, CreditsTransportError> {
        Ok(CreditsReply {
            status: 200,
            body: body.to_string(),
        })
    }

    #[test]
    fn auth_mode_shows_vibe_pool_and_ignores_api_pool() {
        let config = config_with("mistral-auth", "auth-secret", "pasted-secret");
        let selection = selection_from_config(&config).unwrap();
        assert_eq!(selection.api_key, "auth-secret");
        let transport = MapTransport {
            calls: Mutex::new(Vec::new()),
            replies: Mutex::new(vec![
                ok(serde_json::json!({
                    "plan_name": "FREE",
                    "plan_type": "API",
                    "workspace_name": "Personal"
                })),
                ok(serde_json::json!({"plan_name": "INDIVIDUAL", "vibe_payg_enabled": false})),
                ok(serde_json::json!({
                    "vibe_budget": {
                        "usage_percentage": 42.5,
                        "initial_budget": 300.0,
                        "currency": "EUR",
                        "reset_at": "2026-11-01T00:00:00Z"
                    },
                    "api_budget": {
                        "usage_percentage": 3.0,
                        "initial_budget": 50.0,
                        "currency": "USD"
                    }
                })),
            ]),
        };
        let text = render_credits(&selection, &transport);
        assert!(text.contains("Mistral Auth"));
        assert!(text.contains("Plan: FREE (API)"));
        assert!(text.contains("Workspace: Personal"));
        assert!(text.contains("Subscription plan: INDIVIDUAL"));
        assert!(text.contains("Used: 42.5%"));
        assert!(text.contains("Limit: 300 EUR"));
        assert!(text.contains("Reset: 2026-11-01T00:00:00Z"));
        assert!(!text.contains("50"));
        assert!(!text.contains("USD"));
        assert!(!text.contains("auth-secret"));
        assert!(!text.contains("pasted-secret"));
        let calls = transport.calls.lock().unwrap();
        assert!(calls.iter().all(|(_, bearer)| bearer == "auth-secret"));
        assert!(calls.iter().any(|(url, _)| url == "https://api.mistral.ai/v1/billing/budget"));
        assert!(calls.iter().any(|(url, _)| url == "https://console.mistral.ai/api/vibe/whoami"));
    }

    #[test]
    fn api_key_mode_uses_its_own_pool() {
        let config = config_with("mistral", "auth-secret", "pasted-secret");
        let selection = selection_from_config(&config).unwrap();
        assert_eq!(selection.label, "Mistral API Key");
        let transport = MapTransport {
            calls: Mutex::new(Vec::new()),
            replies: Mutex::new(vec![
                ok(serde_json::json!({"plan_name": "SCALE"})),
                ok(serde_json::json!({})),
                ok(serde_json::json!({
                    "vibe_budget": {"usage_percentage": 1.0, "initial_budget": 9.0, "currency": "EUR"},
                    "api_budget": {"usage_percentage": 10.0, "initial_budget": 80.0, "currency": "EUR", "reset_at": "2026-12-01T00:00:00Z"}
                })),
            ]),
        };
        let text = render_credits(&selection, &transport);
        assert!(text.contains("Limit: 80 EUR"));
        assert!(text.contains("Used: 10%"));
        assert!(!text.contains("Limit: 9"));
        assert!(transport.calls.lock().unwrap().iter().all(|(_, b)| b == "pasted-secret"));
    }

    #[test]
    fn missing_pool_unauthorized_and_rate_limit_do_not_invent_a_balance() {
        let config = config_with("mistral", "auth-secret", "pasted-secret");
        let selection = selection_from_config(&config).unwrap();
        let transport = MapTransport {
            calls: Mutex::new(Vec::new()),
            replies: Mutex::new(vec![
                Ok(CreditsReply { status: 401, body: "secret pasted-secret".into() }),
                Ok(CreditsReply { status: 404, body: String::new() }),
                Ok(CreditsReply {
                    status: 403,
                    body: "{\"api_key\":\"pasted-secret\"}".into(),
                }),
            ]),
        };
        let text = render_credits(&selection, &transport);
        assert!(text.contains("HTTP 401"));
        assert!(text.contains("HTTP 404") || text.contains("did not return billing"));
        assert!(text.contains("Balance unavailable"));
        assert!(text.contains("HTTP 403"));
        assert!(text.contains(DASHBOARD_URL));
        assert!(!text.contains("pasted-secret"));
        assert!(!text.contains("Limit: 0"));

        let transport = MapTransport {
            calls: Mutex::new(Vec::new()),
            replies: Mutex::new(vec![
                ok(serde_json::json!({"plan_name": "FREE"})),
                ok(serde_json::json!({"plan_name": "FREE"})),
                Ok(CreditsReply { status: 429, body: "slow down pasted-secret".into() }),
            ]),
        };
        let text = render_credits(&selection, &transport);
        assert!(text.contains("HTTP 429"));
        assert!(!text.contains("pasted-secret"));
        assert!(!text.contains("Limit:"));
    }

    #[test]
    fn other_providers_are_not_given_a_mistral_balance() {
        let config: toml::Value = toml::from_str(
            r#"
[model_providers.setup]
id = "openai"
api_key = "sk-openai"
"#,
        )
        .unwrap();
        let text = selection_from_config(&config).unwrap_err();
        assert!(text.contains("/usage"));
        assert!(!text.contains("sk-openai"));
        assert!(!text.contains("Limit:"));
    }
}
