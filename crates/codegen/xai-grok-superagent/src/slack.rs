use std::collections::BTreeSet;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[derive(Debug, Clone)]
pub struct SlackConfig {
    pub bot_token: String,
    pub app_token: String,
    pub api_base_url: String,
    pub allowed_users: BTreeSet<String>,
    pub allowed_channels: BTreeSet<String>,
}

impl SlackConfig {
    pub fn from_env() -> Result<Self, SlackError> {
        let bot_token = std::env::var("SLACK_BOT_TOKEN")
            .map_err(|_| SlackError::MissingConfiguration("SLACK_BOT_TOKEN"))?;
        let app_token = std::env::var("SLACK_APP_TOKEN")
            .map_err(|_| SlackError::MissingConfiguration("SLACK_APP_TOKEN"))?;
        Ok(Self {
            bot_token,
            app_token,
            api_base_url: std::env::var("SLACK_API_BASE_URL")
                .unwrap_or_else(|_| "https://slack.com/api".to_owned()),
            allowed_users: parse_ids(std::env::var("SLACK_ALLOWED_USERS").ok()),
            allowed_channels: parse_ids(std::env::var("SLACK_ALLOWED_CHANNELS").ok()),
        })
    }

    fn authorized(&self, user_id: &str, channel_id: &str) -> bool {
        (self.allowed_users.is_empty() && self.allowed_channels.is_empty())
            || self.allowed_users.contains(user_id)
            || self.allowed_channels.contains(channel_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackMessage {
    pub envelope_id: String,
    pub channel_id: String,
    pub user_id: String,
    pub thread_ts: Option<String>,
    pub text: String,
}

impl SlackMessage {
    pub fn session_key(&self) -> String {
        format!(
            "agent:main:slack:channel:{}:{}",
            self.channel_id,
            self.thread_ts.as_deref().unwrap_or("root")
        )
    }
}

#[derive(Debug, Error)]
pub enum SlackError {
    #[error("missing Slack configuration: {0}")]
    MissingConfiguration(&'static str),
    #[error("Slack HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Slack WebSocket failed: {0}")]
    WebSocket(String),
    #[error("Slack protocol error: {0}")]
    Protocol(String),
    #[error("Slack JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone)]
pub struct SlackClient {
    http: Client,
    config: SlackConfig,
}

impl SlackClient {
    pub fn new(config: SlackConfig) -> Self {
        Self {
            http: Client::new(),
            config,
        }
    }

    pub async fn send_message(
        &self,
        channel_id: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<(), SlackError> {
        let mut body = json!({"channel": channel_id, "text": text});
        if let Some(thread_ts) = thread_ts {
            body["thread_ts"] = Value::String(thread_ts.to_owned());
        }
        let response: SlackApiResponse<Value> = self
            .http
            .post(format!(
                "{}/chat.postMessage",
                self.config.api_base_url.trim_end_matches('/')
            ))
            .bearer_auth(&self.config.bot_token)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        if response.ok {
            Ok(())
        } else {
            Err(SlackError::Protocol(
                response.error.unwrap_or_else(|| "send failed".to_owned()),
            ))
        }
    }

    pub async fn run_once(&self) -> Result<Vec<SlackMessage>, SlackError> {
        let response: SlackApiResponse<ConnectionInfo> = self
            .http
            .post(format!(
                "{}/apps.connections.open",
                self.config.api_base_url.trim_end_matches('/')
            ))
            .bearer_auth(&self.config.app_token)
            .send()
            .await?
            .json()
            .await?;
        if !response.ok {
            return Err(SlackError::Protocol(
                response
                    .error
                    .unwrap_or_else(|| "connection open failed".to_owned()),
            ));
        }
        let url = response
            .result
            .ok_or_else(|| SlackError::Protocol("missing Socket Mode URL".to_owned()))?
            .url;
        let (socket, _) = connect_async(url)
            .await
            .map_err(|error| SlackError::WebSocket(error.to_string()))?;
        let (mut write, mut read) = socket.split();
        let mut messages = Vec::new();
        let deadline = tokio::time::sleep(Duration::from_secs(30));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => break,
                next = read.next() => {
                    let Some(next) = next else { break; };
                    let message = next.map_err(|error| SlackError::WebSocket(error.to_string()))?;
                    let Message::Text(text) = message else { continue; };
                    let envelope: SlackEnvelope = serde_json::from_str(&text)?;
                    write.send(Message::Text(serde_json::to_string(&json!({"envelope_id": envelope.envelope_id}))?.into())).await.map_err(|error| SlackError::WebSocket(error.to_string()))?;
                    if let Some(message) = normalize_event(&envelope, &self.config) {
                        messages.push(message);
                    }
                }
            }
        }
        Ok(messages)
    }
}

#[derive(Debug, Deserialize)]
struct SlackApiResponse<T> {
    ok: bool,
    result: Option<T>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConnectionInfo {
    url: String,
}

#[derive(Debug, Deserialize)]
struct SlackEnvelope {
    envelope_id: String,
    payload: Value,
}

fn normalize_event(envelope: &SlackEnvelope, config: &SlackConfig) -> Option<SlackMessage> {
    let event = envelope.payload.get("event")?;
    if event.get("type")?.as_str()? != "message" && event.get("type")?.as_str()? != "app_mention" {
        return None;
    }
    if event.get("bot_id").is_some() || event.get("subtype").is_some() {
        return None;
    }
    let channel_id = event.get("channel")?.as_str()?;
    let user_id = event.get("user")?.as_str()?;
    if !config.authorized(user_id, channel_id) {
        return None;
    }
    Some(SlackMessage {
        envelope_id: envelope.envelope_id.clone(),
        channel_id: channel_id.to_owned(),
        user_id: user_id.to_owned(),
        thread_ts: event
            .get("thread_ts")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        text: event.get("text")?.as_str()?.to_owned(),
    })
}

fn parse_ids(value: Option<String>) -> BTreeSet<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{SlackConfig, SlackEnvelope, normalize_event};
    use serde_json::json;

    #[test]
    fn normalizes_authorized_slack_messages_and_session_threads() {
        let config = SlackConfig {
            bot_token: "xoxb".into(),
            app_token: "xapp".into(),
            api_base_url: "https://slack.test/api".into(),
            allowed_users: ["U1".to_owned()].into_iter().collect(),
            allowed_channels: Default::default(),
        };
        let envelope = SlackEnvelope {
            envelope_id: "env-1".into(),
            payload: json!({"event": {"type": "message", "channel": "C1", "user": "U1", "thread_ts": "1.2", "text": "hello"}}),
        };
        let message = normalize_event(&envelope, &config).unwrap();
        assert_eq!(message.session_key(), "agent:main:slack:channel:C1:1.2");
    }
}
