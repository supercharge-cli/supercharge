use std::collections::BTreeSet;

use reqwest::Client;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct SignalConfig {
    pub base_url: String,
    pub account: String,
    pub allowed_senders: BTreeSet<String>,
}

impl SignalConfig {
    pub fn from_env() -> Result<Self, SignalError> {
        Ok(Self {
            base_url: std::env::var("SIGNAL_CLI_BASE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8080".to_owned()),
            account: std::env::var("SIGNAL_ACCOUNT")
                .map_err(|_| SignalError::MissingConfiguration("SIGNAL_ACCOUNT"))?,
            allowed_senders: parse_ids(std::env::var("SIGNAL_ALLOWED_SENDERS").ok()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalMessage {
    pub timestamp: i64,
    pub sender: String,
    pub text: String,
}

impl SignalMessage {
    pub fn session_key(&self) -> String {
        format!("agent:main:signal:private:{}", self.sender)
    }
}

#[derive(Debug, Error)]
pub enum SignalError {
    #[error("missing Signal configuration: {0}")]
    MissingConfiguration(&'static str),
    #[error("Signal REST request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Signal protocol error: {0}")]
    Protocol(String),
}

#[derive(Clone)]
pub struct SignalClient {
    http: Client,
    config: SignalConfig,
}

impl SignalClient {
    pub fn new(config: SignalConfig) -> Self {
        Self {
            http: Client::new(),
            config,
        }
    }

    pub async fn receive_once(&self) -> Result<Vec<SignalMessage>, SignalError> {
        let payload: Value = self
            .http
            .get(format!(
                "{}/v1/receive/{}",
                self.config.base_url.trim_end_matches('/'),
                self.config.account
            ))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let values = payload.as_array().cloned().unwrap_or_default();
        Ok(values
            .into_iter()
            .filter_map(|value| normalize(&value, &self.config.allowed_senders))
            .collect())
    }

    pub async fn send_message(&self, recipient: &str, text: &str) -> Result<(), SignalError> {
        self.http
            .post(format!("{}/v2/send", self.config.base_url.trim_end_matches('/')))
            .json(&serde_json::json!({"number": self.config.account, "recipients": [recipient], "message": text}))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}

fn normalize(value: &Value, allowed: &BTreeSet<String>) -> Option<SignalMessage> {
    let envelope = value.get("envelope")?;
    let sender = envelope.get("sourceNumber").and_then(Value::as_str)?;
    if !allowed.is_empty() && !allowed.contains(sender) {
        return None;
    }
    let data = envelope.get("dataMessage")?;
    let text = data.get("message").and_then(Value::as_str)?.to_owned();
    let timestamp = envelope
        .get("timestamp")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    Some(SignalMessage {
        timestamp,
        sender: sender.to_owned(),
        text,
    })
}

fn parse_ids(value: Option<String>) -> BTreeSet<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{SignalConfig, normalize};
    use serde_json::json;

    #[test]
    fn normalizes_allowlisted_signal_messages() {
        let config = SignalConfig {
            base_url: "http://signal.test".into(),
            account: "+100".into(),
            allowed_senders: ["+200".to_owned()].into_iter().collect(),
        };
        let value = json!({"envelope":{"sourceNumber":"+200","timestamp":42,"dataMessage":{"message":"hello"}}});
        assert_eq!(
            normalize(&value, &config.allowed_senders)
                .unwrap()
                .session_key(),
            "agent:main:signal:private:+200"
        );
    }
}
