use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::gateway::{GatewayConfig, GatewayMessage, GatewayStateError, GatewayStateStore};

#[derive(Debug, Clone)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub api_base_url: String,
    pub poll_timeout_seconds: u64,
    pub webhook_secret: Option<String>,
    pub gateway: GatewayConfig,
}

impl TelegramConfig {
    pub fn from_env() -> Result<Self, TelegramError> {
        let bot_token = std::env::var("TELEGRAM_BOT_TOKEN")
            .map_err(|_| TelegramError::MissingConfiguration("TELEGRAM_BOT_TOKEN"))?;
        let allowed_users = parse_ids(std::env::var("TELEGRAM_ALLOWED_USERS").ok());
        let allowed_chats = parse_ids(std::env::var("TELEGRAM_ALLOWED_CHATS").ok());
        Ok(Self {
            bot_token,
            api_base_url: std::env::var("TELEGRAM_API_BASE_URL")
                .unwrap_or_else(|_| "https://api.telegram.org".to_owned()),
            poll_timeout_seconds: std::env::var("TELEGRAM_POLL_TIMEOUT_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(30)
                .clamp(1, 50),
            webhook_secret: std::env::var("TELEGRAM_WEBHOOK_SECRET").ok(),
            gateway: GatewayConfig {
                allowed_users,
                allowed_chats,
                ..GatewayConfig::default()
            },
        })
    }

    fn endpoint(&self, method: &str) -> String {
        format!(
            "{}/bot{}/{}",
            self.api_base_url.trim_end_matches('/'),
            self.bot_token,
            method
        )
    }
}

#[derive(Debug, Error)]
pub enum TelegramError {
    #[error("missing Telegram configuration: {0}")]
    MissingConfiguration(&'static str),
    #[error("Telegram HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Telegram API returned an error: {0}")]
    Api(String),
    #[error("Telegram response decoding failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("gateway state failed: {0}")]
    State(#[from] GatewayStateError),
}

#[derive(Clone)]
pub struct TelegramClient {
    http: Client,
    config: TelegramConfig,
}

impl TelegramClient {
    pub fn new(config: TelegramConfig) -> Self {
        Self {
            http: Client::new(),
            config,
        }
    }

    pub async fn get_updates(
        &self,
        offset: Option<i64>,
    ) -> Result<Vec<TelegramUpdate>, TelegramError> {
        let payload = GetUpdatesRequest {
            offset,
            timeout: self.config.poll_timeout_seconds,
            allowed_updates: vec!["message".to_owned()],
        };
        let response: TelegramResponse<Vec<TelegramUpdate>> = self
            .http
            .post(self.config.endpoint("getUpdates"))
            .timeout(Duration::from_secs(self.config.poll_timeout_seconds + 10))
            .json(&payload)
            .send()
            .await?
            .json()
            .await?;
        response.into_result()
    }

    pub async fn send_message(&self, chat_id: i64, text: &str) -> Result<(), TelegramError> {
        let response: TelegramResponse<serde_json::Value> = self
            .http
            .post(self.config.endpoint("sendMessage"))
            .json(&SendMessageRequest { chat_id, text })
            .send()
            .await?
            .json()
            .await?;
        response.into_result().map(|_| ())
    }

    pub fn webhook_secret(&self) -> Option<&str> {
        self.config.webhook_secret.as_deref()
    }

    pub fn normalize_update(&self, update: TelegramUpdate) -> Option<GatewayMessage> {
        let message = update.message?;
        let normalized = GatewayMessage {
            platform: "telegram".to_owned(),
            update_id: update.update_id,
            chat_id: message.chat.id,
            user_id: message.from.map(|user| user.id),
            thread_id: message.message_thread_id,
            text: message.text.unwrap_or_default(),
        };
        self.config
            .gateway
            .authorized(normalized.user_id, normalized.chat_id)
            .then_some(normalized)
    }

    pub async fn poll_once(
        &self,
        state: &mut GatewayStateStore,
    ) -> Result<Vec<GatewayMessage>, TelegramError> {
        let offset = state.update_offset().map(|value| value + 1);
        let updates = self.get_updates(offset).await?;
        let mut messages = Vec::new();
        let mut next_offset = state.update_offset();
        for update in updates {
            next_offset =
                Some(next_offset.map_or(update.update_id, |value| value.max(update.update_id)));
            if let Some(message) = self.normalize_update(update) {
                messages.push(message);
            }
        }
        if let Some(offset) = next_offset {
            state.set_update_offset(offset)?;
        }
        Ok(messages)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramMessage {
    pub message_id: i64,
    pub message_thread_id: Option<i64>,
    pub from: Option<TelegramUser>,
    pub chat: TelegramChat,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramUser {
    pub id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
}

#[derive(Debug, Serialize)]
struct GetUpdatesRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<i64>,
    timeout: u64,
    allowed_updates: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SendMessageRequest<'a> {
    chat_id: i64,
    text: &'a str,
}

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    description: Option<String>,
    result: Option<T>,
}

impl<T> TelegramResponse<T> {
    fn into_result(self) -> Result<T, TelegramError> {
        if self.ok {
            self.result
                .ok_or_else(|| TelegramError::Api("missing result".to_owned()))
        } else {
            Err(TelegramError::Api(
                self.description
                    .unwrap_or_else(|| "unknown Telegram error".to_owned()),
            ))
        }
    }
}

fn parse_ids(value: Option<String>) -> std::collections::BTreeSet<i64> {
    value
        .unwrap_or_default()
        .split(',')
        .filter_map(|entry| entry.trim().parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use httpmock::prelude::*;
    use tempfile::tempdir;

    use super::{TelegramClient, TelegramConfig};
    use crate::gateway::{GatewayConfig, GatewayStateStore};

    #[tokio::test]
    async fn polling_normalizes_authorized_messages_and_advances_offset() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.method(POST).path("/botsecret/getUpdates");
                then.status(200).json_body(serde_json::json!({
                    "ok": true,
                    "result": [{
                        "update_id": 10,
                        "message": {
                            "message_id": 1,
                            "from": {"id": 7},
                            "chat": {"id": 42},
                            "text": "hello"
                        }
                    }]
                }));
            })
            .await;
        let config = TelegramConfig {
            bot_token: "secret".into(),
            api_base_url: server.base_url(),
            poll_timeout_seconds: 1,
            webhook_secret: None,
            gateway: GatewayConfig {
                allowed_users: [7].into_iter().collect(),
                ..GatewayConfig::default()
            },
        };
        let client = TelegramClient::new(config);
        let dir = tempdir().unwrap();
        let mut state = GatewayStateStore::load(dir.path().join("state.json")).unwrap();
        let messages = client.poll_once(&mut state).await.unwrap();
        assert_eq!(messages[0].text, "hello");
        assert_eq!(
            messages[0].session_key().as_str(),
            "agent:main:telegram:chat:42"
        );
        assert_eq!(state.update_offset(), Some(10));
        mock.assert_async().await;
    }
}
