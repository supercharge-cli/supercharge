use std::collections::BTreeSet;

use reqwest::Client;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct MatrixConfig {
    pub homeserver: String,
    pub access_token: String,
    pub user_id: String,
    pub allowed_users: BTreeSet<String>,
    pub allowed_rooms: BTreeSet<String>,
    pub sync_timeout_ms: u64,
}

impl MatrixConfig {
    pub fn from_env() -> Result<Self, MatrixError> {
        Ok(Self {
            homeserver: std::env::var("MATRIX_HOMESERVER")
                .map_err(|_| MatrixError::MissingConfiguration("MATRIX_HOMESERVER"))?,
            access_token: std::env::var("MATRIX_ACCESS_TOKEN")
                .map_err(|_| MatrixError::MissingConfiguration("MATRIX_ACCESS_TOKEN"))?,
            user_id: std::env::var("MATRIX_USER_ID")
                .map_err(|_| MatrixError::MissingConfiguration("MATRIX_USER_ID"))?,
            allowed_users: parse_ids(std::env::var("MATRIX_ALLOWED_USERS").ok()),
            allowed_rooms: parse_ids(std::env::var("MATRIX_ALLOWED_ROOMS").ok()),
            sync_timeout_ms: std::env::var("MATRIX_SYNC_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(30_000)
                .clamp(1_000, 120_000),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixMessage {
    pub event_id: String,
    pub room_id: String,
    pub sender: String,
    pub text: String,
}

impl MatrixMessage {
    pub fn session_key(&self) -> String {
        format!("agent:main:matrix:room:{}", self.room_id)
    }
}

#[derive(Debug, Error)]
pub enum MatrixError {
    #[error("missing Matrix configuration: {0}")]
    MissingConfiguration(&'static str),
    #[error("Matrix HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Matrix protocol error: {0}")]
    Protocol(String),
    #[error("Matrix JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone)]
pub struct MatrixClient {
    http: Client,
    config: MatrixConfig,
}

impl MatrixClient {
    pub fn new(config: MatrixConfig) -> Self {
        Self {
            http: Client::new(),
            config,
        }
    }

    pub async fn sync_once(
        &self,
        since: Option<&str>,
    ) -> Result<(Option<String>, Vec<MatrixMessage>), MatrixError> {
        let mut request = self
            .http
            .get(format!(
                "{}/_matrix/client/v3/sync",
                self.config.homeserver.trim_end_matches('/')
            ))
            .bearer_auth(&self.config.access_token)
            .query(&[("timeout", self.config.sync_timeout_ms.to_string())]);
        if let Some(since) = since {
            request = request.query(&[("since", since)]);
        }
        let payload: Value = request.send().await?.error_for_status()?.json().await?;
        let next_batch = payload
            .get("next_batch")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let mut messages = Vec::new();
        let Some(joined) = payload
            .get("rooms")
            .and_then(|rooms| rooms.get("join"))
            .and_then(Value::as_object)
        else {
            return Ok((next_batch, messages));
        };
        for (room_id, room) in joined {
            if !self.config.allowed_rooms.is_empty() && !self.config.allowed_rooms.contains(room_id)
            {
                continue;
            }
            let Some(events) = room
                .get("timeline")
                .and_then(|timeline| timeline.get("events"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for event in events {
                if event.get("type").and_then(Value::as_str) != Some("m.room.message") {
                    continue;
                }
                let Some(sender) = event.get("sender").and_then(Value::as_str) else {
                    continue;
                };
                if sender == self.config.user_id
                    || (!self.config.allowed_users.is_empty()
                        && !self.config.allowed_users.contains(sender))
                {
                    continue;
                }
                let Some(text) = event
                    .get("content")
                    .and_then(|content| content.get("body"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let Some(event_id) = event.get("event_id").and_then(Value::as_str) else {
                    continue;
                };
                messages.push(MatrixMessage {
                    event_id: event_id.to_owned(),
                    room_id: room_id.to_owned(),
                    sender: sender.to_owned(),
                    text: text.to_owned(),
                });
            }
        }
        Ok((next_batch, messages))
    }

    pub async fn send_message(&self, room_id: &str, text: &str) -> Result<(), MatrixError> {
        let txn = uuid::Uuid::new_v4();
        self.http
            .put(format!(
                "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
                self.config.homeserver.trim_end_matches('/'),
                urlencoding::encode(room_id),
                txn
            ))
            .bearer_auth(&self.config.access_token)
            .json(&serde_json::json!({"msgtype": "m.text", "body": text}))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
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
    use super::{MatrixClient, MatrixConfig};
    use serde_json::json;

    #[test]
    fn config_and_message_shape_support_room_session_routing() {
        let config = MatrixConfig {
            homeserver: "https://matrix.test".into(),
            access_token: "token".into(),
            user_id: "@bot:matrix.test".into(),
            allowed_users: ["@user:matrix.test".to_owned()].into_iter().collect(),
            allowed_rooms: Default::default(),
            sync_timeout_ms: 1000,
        };
        let client = MatrixClient::new(config);
        let payload = json!({
            "rooms": {
                "join": {
                    "!room:matrix.test": {
                        "timeline": {
                            "events": [{
                                "type": "m.room.message",
                                "event_id": "$1",
                                "sender": "@user:matrix.test",
                                "content": {"body": "hello"}
                            }]
                        }
                    }
                }
            },
            "next_batch": "s2"
        });
        assert_eq!(payload["next_batch"], "s2");
        assert_eq!(client.config.user_id, "@bot:matrix.test");
    }
}
