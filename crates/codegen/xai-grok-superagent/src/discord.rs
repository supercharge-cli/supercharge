use std::{collections::BTreeSet, time::Duration};

use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::gateway::{GatewayConfig, GatewayMessage};

const OP_DISPATCH: i64 = 0;
const OP_HEARTBEAT: i64 = 1;
const OP_IDENTIFY: i64 = 2;
const OP_RECONNECT: i64 = 7;
const OP_INVALID_SESSION: i64 = 9;
const OP_HELLO: i64 = 10;

const INTENT_GUILDS: u64 = 1 << 0;
const INTENT_GUILD_MESSAGES: u64 = 1 << 9;
const INTENT_MESSAGE_CONTENT: u64 = 1 << 15;

#[derive(Debug, Clone)]
pub struct DiscordConfig {
    pub bot_token: String,
    pub api_base_url: String,
    pub gateway_url: String,
    pub intents: u64,
    pub gateway: GatewayConfig,
    pub allowed_users: BTreeSet<String>,
    pub allowed_channels: BTreeSet<String>,
}

impl DiscordConfig {
    pub fn from_env() -> Result<Self, DiscordError> {
        let bot_token = std::env::var("DISCORD_BOT_TOKEN")
            .map_err(|_| DiscordError::MissingConfiguration("DISCORD_BOT_TOKEN"))?;
        Ok(Self {
            bot_token,
            api_base_url: std::env::var("DISCORD_API_BASE_URL")
                .unwrap_or_else(|_| "https://discord.com/api/v10".to_owned()),
            gateway_url: std::env::var("DISCORD_GATEWAY_URL")
                .unwrap_or_else(|_| "wss://gateway.discord.gg/?v=10&encoding=json".to_owned()),
            intents: INTENT_GUILDS | INTENT_GUILD_MESSAGES | INTENT_MESSAGE_CONTENT,
            gateway: GatewayConfig {
                require_user_or_chat_allowlist: false,
                require_mention_in_groups: true,
                ..GatewayConfig::default()
            },
            allowed_users: parse_strings(std::env::var("DISCORD_ALLOWED_USERS").ok()),
            allowed_channels: parse_strings(std::env::var("DISCORD_ALLOWED_CHANNELS").ok()),
        })
    }

    pub fn authorized(&self, user_id: &str, channel_id: &str) -> bool {
        (self.allowed_users.is_empty() && self.allowed_channels.is_empty())
            || self.allowed_users.contains(user_id)
            || self.allowed_channels.contains(channel_id)
    }
}

#[derive(Debug, Error)]
pub enum DiscordError {
    #[error("missing Discord configuration: {0}")]
    MissingConfiguration(&'static str),
    #[error("Discord HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Discord WebSocket failed: {0}")]
    WebSocket(String),
    #[error("Discord protocol error: {0}")]
    Protocol(String),
    #[error("Discord JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone)]
pub struct DiscordClient {
    http: Client,
    config: DiscordConfig,
}

impl DiscordClient {
    pub fn new(config: DiscordConfig) -> Self {
        Self {
            http: Client::new(),
            config,
        }
    }

    pub async fn send_message(&self, channel_id: &str, content: &str) -> Result<(), DiscordError> {
        let response = self
            .http
            .post(format!(
                "{}/channels/{channel_id}/messages",
                self.config.api_base_url.trim_end_matches('/')
            ))
            .bearer_auth(&self.config.bot_token)
            .json(&json!({ "content": content }))
            .send()
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(DiscordError::Protocol(format!(
                "send message returned {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )))
        }
    }

    pub async fn run_once(&self) -> Result<Vec<GatewayMessage>, DiscordError> {
        let (socket, _) = connect_async(&self.config.gateway_url)
            .await
            .map_err(|error| DiscordError::WebSocket(error.to_string()))?;
        let (mut write, mut read) = socket.split();
        let hello = read
            .next()
            .await
            .ok_or_else(|| DiscordError::Protocol("gateway closed before Hello".to_owned()))?
            .map_err(|error| DiscordError::WebSocket(error.to_string()))?;
        let hello: GatewayPayload = decode_message(hello)?;
        if hello.op != OP_HELLO {
            return Err(DiscordError::Protocol(format!(
                "expected Hello, got op {}",
                hello.op
            )));
        }
        let heartbeat_ms = hello
            .d
            .get("heartbeat_interval")
            .and_then(Value::as_u64)
            .ok_or_else(|| DiscordError::Protocol("Hello missing heartbeat_interval".to_owned()))?;
        write
            .send(Message::Text(
                serde_json::to_string(&GatewayPayload::identify(self.config.intents))?.into(),
            ))
            .await
            .map_err(|error| DiscordError::WebSocket(error.to_string()))?;

        let mut heartbeat = tokio::time::interval(Duration::from_millis(heartbeat_ms.max(1)));
        heartbeat.tick().await;
        let mut messages = Vec::new();
        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    write.send(Message::Text(serde_json::to_string(&GatewayPayload {
                        op: OP_HEARTBEAT,
                        d: Value::Null,
                        s: None,
                        t: None,
                    })?.into())).await.map_err(|error| DiscordError::WebSocket(error.to_string()))?;
                }
                next = read.next() => {
                    let Some(next) = next else { break; };
                    let payload = decode_message(next.map_err(|error| DiscordError::WebSocket(error.to_string()))?)?;
                    if payload.op == OP_RECONNECT || payload.op == OP_INVALID_SESSION {
                        break;
                    }
                    if payload.op == OP_DISPATCH
                        && payload.t.as_deref() == Some("MESSAGE_CREATE")
                        && let Some(message) = normalize_message(payload.d, &self.config)
                    {
                        messages.push(message);
                    }
                }
            }
            if messages.len() >= 100 {
                break;
            }
        }
        Ok(messages)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GatewayPayload {
    op: i64,
    d: Value,
    s: Option<i64>,
    t: Option<String>,
}

impl GatewayPayload {
    fn identify(intents: u64) -> Self {
        Self {
            op: OP_IDENTIFY,
            d: json!({
                "token": "redacted-at-wire-serialization",
                "properties": {
                    "os": "linux",
                    "browser": "grok-superagent",
                    "device": "grok-superagent"
                },
                "intents": intents
            }),
            s: None,
            t: None,
        }
    }
}

fn decode_message(message: Message) -> Result<GatewayPayload, DiscordError> {
    match message {
        Message::Text(text) => Ok(serde_json::from_str(&text)?),
        Message::Binary(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Message::Ping(_) | Message::Pong(_) => Err(DiscordError::Protocol(
            "unexpected control frame before payload".to_owned(),
        )),
        Message::Close(_) => Err(DiscordError::Protocol("gateway closed".to_owned())),
        Message::Frame(_) => Err(DiscordError::Protocol(
            "raw frame is unsupported".to_owned(),
        )),
    }
}

fn normalize_message(data: Value, config: &DiscordConfig) -> Option<GatewayMessage> {
    let channel_id = data.get("channel_id")?.as_str()?;
    let author = data.get("author")?;
    let user_id = author.get("id")?.as_str()?;
    if author.get("bot").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    if !config.authorized(user_id, channel_id) {
        return None;
    }
    let chat_id = channel_id.parse().ok()?;
    let user_id_number = user_id.parse().ok();
    let thread_id = data
        .get("message_reference")
        .and_then(|reference| reference.get("message_id"))
        .and_then(Value::as_str)
        .and_then(|value| value.parse().ok());
    Some(GatewayMessage {
        platform: "discord".to_owned(),
        update_id: data.get("id")?.as_str()?.parse().ok()?,
        chat_id,
        user_id: user_id_number,
        thread_id,
        text: data.get("content")?.as_str()?.to_owned(),
    })
}

fn parse_strings(value: Option<String>) -> BTreeSet<String> {
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
    use httpmock::prelude::*;
    use serde_json::json;

    use super::{DiscordClient, DiscordConfig, GatewayPayload, OP_IDENTIFY, normalize_message};

    #[test]
    fn identify_uses_message_content_and_standard_message_intents() {
        let payload = GatewayPayload::identify(33281);
        assert_eq!(payload.op, OP_IDENTIFY);
        assert_eq!(payload.d["intents"], json!(33281));
        assert_eq!(payload.d["token"], json!("redacted-at-wire-serialization"));
    }

    #[tokio::test]
    async fn sends_message_through_discord_rest_api() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/api/v10/channels/42/messages")
                    .json_body(json!({"content": "hello"}));
                then.status(200).json_body(json!({"id": "9"}));
            })
            .await;
        let client = DiscordClient::new(DiscordConfig {
            bot_token: "secret".into(),
            api_base_url: format!("{}/api/v10", server.base_url()),
            gateway_url: "wss://discord.test".into(),
            intents: 0,
            gateway: Default::default(),
            allowed_users: Default::default(),
            allowed_channels: Default::default(),
        });
        client.send_message("42", "hello").await.unwrap();
        mock.assert_async().await;
    }

    #[test]
    fn message_normalization_rejects_bots_and_unallowlisted_channels() {
        let config = DiscordConfig {
            bot_token: "secret".into(),
            api_base_url: "https://discord.test/api/v10".into(),
            gateway_url: "wss://discord.test".into(),
            intents: 0,
            gateway: Default::default(),
            allowed_users: ["7".to_owned()].into_iter().collect(),
            allowed_channels: Default::default(),
        };
        let message = json!({
            "id": "9",
            "channel_id": "42",
            "author": {"id": "7", "bot": false},
            "content": "hello"
        });
        assert_eq!(normalize_message(message, &config).unwrap().text, "hello");
        assert!(
            normalize_message(
                json!({
                    "id": "10",
                    "channel_id": "42",
                    "author": {"id": "8", "bot": false},
                    "content": "blocked"
                }),
                &config
            )
            .is_none()
        );
        assert!(
            normalize_message(
                json!({
                    "id": "11",
                    "channel_id": "42",
                    "author": {"id": "7", "bot": true},
                    "content": "loop"
                }),
                &config
            )
            .is_none()
        );
    }
}
