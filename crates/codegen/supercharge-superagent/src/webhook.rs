use std::collections::BTreeSet;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebhookEvent {
    pub event_id: String,
    pub source: String,
    pub user_id: Option<String>,
    pub chat_id: Option<String>,
    pub thread_id: Option<String>,
    pub text: String,
}

impl WebhookEvent {
    pub fn session_key(&self) -> String {
        format!(
            "agent:main:webhook:{}:{}",
            self.source,
            self.thread_id
                .as_deref()
                .or(self.chat_id.as_deref())
                .unwrap_or("root")
        )
    }
}

#[derive(Debug, Clone)]
pub struct WebhookConfig {
    pub secret: Option<String>,
    pub allowed_sources: BTreeSet<String>,
}

#[derive(Debug, Error)]
pub enum WebhookError {
    #[error("webhook authentication failed")]
    Unauthorized,
    #[error("webhook JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("webhook HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("webhook delivery failed: {0}")]
    Delivery(String),
}

#[derive(Clone)]
pub struct WebhookClient {
    http: Client,
}

impl WebhookClient {
    pub fn new() -> Self {
        Self {
            http: Client::new(),
        }
    }

    pub fn verify_secret(
        &self,
        config: &WebhookConfig,
        provided: Option<&str>,
    ) -> Result<(), WebhookError> {
        match (&config.secret, provided) {
            (Some(expected), Some(actual))
                if constant_time_equal(expected.as_bytes(), actual.as_bytes()) =>
            {
                Ok(())
            }
            (None, _) => Ok(()),
            _ => Err(WebhookError::Unauthorized),
        }
    }

    pub fn parse_event(
        &self,
        config: &WebhookConfig,
        payload: &[u8],
    ) -> Result<Option<WebhookEvent>, WebhookError> {
        let event: WebhookEvent = serde_json::from_slice(payload)?;
        if !config.allowed_sources.is_empty() && !config.allowed_sources.contains(&event.source) {
            return Ok(None);
        }
        Ok(Some(event))
    }

    pub async fn deliver_json(
        &self,
        url: &str,
        secret: Option<&str>,
        payload: &impl Serialize,
    ) -> Result<(), WebhookError> {
        let mut request = self.http.post(url).json(payload);
        if let Some(secret) = secret {
            request = request.header("x-superagent-webhook-secret", secret);
        }
        let response = request.send().await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(WebhookError::Delivery(
                response.text().await.unwrap_or_default(),
            ))
        }
    }
}

impl Default for WebhookClient {
    fn default() -> Self {
        Self::new()
    }
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::{WebhookClient, WebhookConfig};

    #[test]
    fn authenticates_and_filters_normalized_events() {
        let client = WebhookClient::new();
        let config = WebhookConfig {
            secret: Some("secret".into()),
            allowed_sources: ["internal".to_owned()].into_iter().collect(),
        };
        assert!(client.verify_secret(&config, Some("secret")).is_ok());
        let event = br#"{"event_id":"evt-1","source":"internal","user_id":"u1","chat_id":"c1","thread_id":null,"text":"hello"}"#;
        assert_eq!(
            client
                .parse_event(&config, event)
                .unwrap()
                .unwrap()
                .session_key(),
            "agent:main:webhook:internal:c1"
        );
    }
}
