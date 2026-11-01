//! Lightweight PostHog HTTP capture client.
//!
//! Mirrors `supercharge-mixpanel`: a minimal reqwest-only client covering just
//! the capture API, with secret scrubbing before send.

#![deny(clippy::indexing_slicing)]

use std::collections::HashMap;

/// PostHog client for sending capture events.
#[derive(Clone)]
pub struct Posthog {
    api_key: String,
    host: String,
    client: reqwest::Client,
}

/// Error type for PostHog operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const DEFAULT_HOST: &str = "https://us.i.posthog.com";

impl Posthog {
    /// Create a new PostHog client with the given project API key.
    #[allow(clippy::disallowed_methods)] // transport-neutral crate; the CLI injects a policy client via with_client
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_client(api_key, reqwest::Client::new())
    }

    /// Create a new PostHog client with a shared reqwest client.
    pub fn with_client(api_key: impl Into<String>, client: reqwest::Client) -> Self {
        Self {
            api_key: api_key.into(),
            host: DEFAULT_HOST.to_owned(),
            client,
        }
    }

    /// Point the client at a different ingestion host, e.g. `https://eu.i.posthog.com`.
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
    }

    /// Scrub property string values in place, then build the capture body. The
    /// project `api_key` is top-level — outside properties — so scrubbing
    /// cannot redact it.
    fn prepare_body(
        &self,
        event: &str,
        mut properties: HashMap<String, serde_json::Value>,
        timestamp: Option<String>,
    ) -> serde_json::Value {
        for v in properties.values_mut() {
            supercharge_secrets::redact_json_string_values(v);
        }
        let mut body = serde_json::json!({
            "api_key": self.api_key,
            "event": event,
            "properties": properties,
        });
        if let Some(ts) = timestamp {
            body["timestamp"] = serde_json::json!(ts);
        }
        body
    }

    /// Capture an event. Properties should include `distinct_id`; `timestamp`
    /// is an optional ISO 8601 string (PostHog rejects future-dated or
    /// older-than-5-years timestamps).
    pub async fn capture(
        &self,
        event: &str,
        properties: Option<HashMap<String, serde_json::Value>>,
        timestamp: Option<String>,
    ) -> Result<(), Error> {
        let body = self.prepare_body(event, properties.unwrap_or_default(), timestamp);
        self.client
            .post(capture_url(&self.host))
            .timeout(REQUEST_TIMEOUT)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}

/// Trailing-slash-tolerant ingestion URL for the single-event capture endpoint.
fn capture_url(host: &str) -> String {
    format!("{}/capture/", host.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The API key is deliberately Bearer-shaped: it would be redacted if the scrubber ran after
    /// injection. The `error` value catches the inverse regression: if the scrub loop is dropped,
    /// the user-supplied Bearer leaks.
    #[test]
    fn prepare_body_scrubs_properties_but_keeps_api_key() {
        let api_key = "Bearer phc-fake-api-key-abcdef0123456789";
        let ph = Posthog::new(api_key);

        let mut props = HashMap::new();
        props.insert("distinct_id".into(), serde_json::json!("user-1"));
        props.insert("error".into(), "Bearer abcdef0123456789abcdef".into());

        let body = ph.prepare_body(
            "test_event",
            props,
            Some("2026-10-29T00:00:00.000Z".to_owned()),
        );

        assert_eq!(
            body.get("api_key"),
            Some(&serde_json::json!(api_key)),
            "api key must not be redacted"
        );
        assert_eq!(body.get("event"), Some(&serde_json::json!("test_event")));
        assert_eq!(
            body.get("timestamp"),
            Some(&serde_json::json!("2026-10-29T00:00:00.000Z")),
        );
        let error = body
            .get("properties")
            .and_then(|p| p.get("error"))
            .and_then(|v| v.as_str())
            .expect("error property must stay a string");
        assert!(
            !error.contains("abcdef0123456789abcdef"),
            "secret leaked: {error}"
        );
    }

    #[test]
    fn capture_url_trims_trailing_slash() {
        assert_eq!(
            capture_url("https://us.i.posthog.com/"),
            "https://us.i.posthog.com/capture/"
        );
        assert_eq!(
            capture_url("https://eu.i.posthog.com"),
            "https://eu.i.posthog.com/capture/"
        );
    }
}
