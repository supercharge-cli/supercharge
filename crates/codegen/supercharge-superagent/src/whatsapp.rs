use std::collections::BTreeSet;

use reqwest::{Client, multipart};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct WhatsAppConfig {
    pub access_token: String,
    pub phone_number_id: String,
    pub verify_token: String,
    pub app_secret: Option<String>,
    pub graph_base_url: String,
    pub allowed_users: BTreeSet<String>,
}

impl WhatsAppConfig {
    pub fn from_env() -> Result<Self, WhatsAppError> {
        Ok(Self {
            access_token: std::env::var("WHATSAPP_ACCESS_TOKEN")
                .map_err(|_| WhatsAppError::MissingConfiguration("WHATSAPP_ACCESS_TOKEN"))?,
            phone_number_id: std::env::var("WHATSAPP_PHONE_NUMBER_ID")
                .map_err(|_| WhatsAppError::MissingConfiguration("WHATSAPP_PHONE_NUMBER_ID"))?,
            verify_token: std::env::var("WHATSAPP_VERIFY_TOKEN")
                .map_err(|_| WhatsAppError::MissingConfiguration("WHATSAPP_VERIFY_TOKEN"))?,
            app_secret: std::env::var("WHATSAPP_APP_SECRET").ok(),
            graph_base_url: std::env::var("WHATSAPP_GRAPH_BASE_URL")
                .unwrap_or_else(|_| "https://graph.facebook.com/v23.0".to_owned()),
            allowed_users: parse_ids(std::env::var("WHATSAPP_ALLOWED_USERS").ok()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhatsAppMessage {
    pub message_id: String,
    pub user_id: String,
    pub text: String,
}

impl WhatsAppMessage {
    pub fn session_key(&self) -> String {
        format!("agent:main:whatsapp:private:{}", self.user_id)
    }
}

#[derive(Debug, Error)]
pub enum WhatsAppError {
    #[error("missing WhatsApp configuration: {0}")]
    MissingConfiguration(&'static str),
    #[error("WhatsApp HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("WhatsApp signature verification failed")]
    InvalidSignature,
    #[error("WhatsApp Graph API returned an error: {0}")]
    Api(String),
}

#[derive(Clone)]
pub struct WhatsAppClient {
    http: Client,
    config: WhatsAppConfig,
}

impl WhatsAppClient {
    pub fn new(config: WhatsAppConfig) -> Self {
        Self {
            http: crate::default_http_client(),
            config,
        }
    }

    pub fn verify_challenge(
        &self,
        mode: Option<&str>,
        token: Option<&str>,
        challenge: Option<&str>,
    ) -> Option<String> {
        (mode == Some("subscribe") && token == Some(self.config.verify_token.as_str()))
            .then(|| challenge.unwrap_or_default().to_owned())
    }

    pub fn verify_signature(
        &self,
        body: &[u8],
        signature_header: Option<&str>,
    ) -> Result<(), WhatsAppError> {
        let Some(secret) = &self.config.app_secret else {
            return Ok(());
        };
        let Some(signature) = signature_header.and_then(|value| value.strip_prefix("sha256="))
        else {
            return Err(WhatsAppError::InvalidSignature);
        };
        let expected = hmac_sha256_hex(secret.as_bytes(), body);
        if constant_time_equal(signature.as_bytes(), expected.as_bytes()) {
            Ok(())
        } else {
            Err(WhatsAppError::InvalidSignature)
        }
    }

    pub fn parse_messages(&self, payload: &Value) -> Vec<WhatsAppMessage> {
        let mut result = Vec::new();
        let Some(entries) = payload.get("entry").and_then(Value::as_array) else {
            return result;
        };
        for entry in entries {
            let Some(changes) = entry.get("changes").and_then(Value::as_array) else {
                continue;
            };
            for change in changes {
                let Some(messages) = change
                    .get("value")
                    .and_then(|value| value.get("messages"))
                    .and_then(Value::as_array)
                else {
                    continue;
                };
                for message in messages {
                    let Some(user_id) = message.get("from").and_then(Value::as_str) else {
                        continue;
                    };
                    if !self.config.allowed_users.is_empty()
                        && !self.config.allowed_users.contains(user_id)
                    {
                        continue;
                    }
                    if message.get("type").and_then(Value::as_str) != Some("text") {
                        continue;
                    }
                    let Some(text) = message
                        .get("text")
                        .and_then(|text| text.get("body"))
                        .and_then(Value::as_str)
                    else {
                        continue;
                    };
                    let Some(message_id) = message.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    result.push(WhatsAppMessage {
                        message_id: message_id.to_owned(),
                        user_id: user_id.to_owned(),
                        text: text.to_owned(),
                    });
                }
            }
        }
        result
    }

    pub async fn send_media(
        &self,
        recipient: &str,
        bytes: Vec<u8>,
        mime_type: &str,
        filename: &str,
    ) -> Result<(), WhatsAppError> {
        let part = multipart::Part::bytes(bytes)
            .file_name(filename.to_owned())
            .mime_str(mime_type)?;
        let upload = self
            .http
            .post(format!(
                "{}/{}/media",
                self.config.graph_base_url.trim_end_matches('/'),
                self.config.phone_number_id
            ))
            .bearer_auth(&self.config.access_token)
            .multipart(
                multipart::Form::new()
                    .text("messaging_product", "whatsapp")
                    .text("type", mime_type.to_owned())
                    .part("file", part),
            )
            .send()
            .await?;
        if !upload.status().is_success() {
            return Err(WhatsAppError::Api(upload.text().await.unwrap_or_default()));
        }
        let media_id = upload
            .json::<Value>()
            .await?
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| WhatsAppError::Api("media upload response omitted id".to_owned()))?
            .to_owned();
        let response = self
            .http
            .post(format!(
                "{}/{}/messages",
                self.config.graph_base_url.trim_end_matches('/'),
                self.config.phone_number_id
            ))
            .bearer_auth(&self.config.access_token)
            .json(&serde_json::json!({
                "messaging_product": "whatsapp",
                "to": recipient,
                "type": "document",
                "document": {"id": media_id, "filename": filename}
            }))
            .send()
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(WhatsAppError::Api(
                response.text().await.unwrap_or_default(),
            ))
        }
    }

    pub async fn send_text(&self, recipient: &str, text: &str) -> Result<(), WhatsAppError> {
        let response = self
            .http
            .post(format!(
                "{}/{}/messages",
                self.config.graph_base_url.trim_end_matches('/'),
                self.config.phone_number_id
            ))
            .bearer_auth(&self.config.access_token)
            .json(&serde_json::json!({
                "messaging_product": "whatsapp",
                "to": recipient,
                "type": "text",
                "text": {"body": text}
            }))
            .send()
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(WhatsAppError::Api(
                response.text().await.unwrap_or_default(),
            ))
        }
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

fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    let mut padded = [0_u8; 64];
    if key.len() > 64 {
        let digest = Sha256::digest(key);
        padded[..32].copy_from_slice(&digest);
    } else {
        padded[..key.len()].copy_from_slice(key);
    }
    let mut inner = [0_u8; 64];
    let mut outer = [0_u8; 64];
    for index in 0..64 {
        inner[index] = padded[index] ^ 0x36;
        outer[index] = padded[index] ^ 0x5c;
    }
    let mut inner_input = Vec::with_capacity(64 + message.len());
    inner_input.extend_from_slice(&inner);
    inner_input.extend_from_slice(message);
    let inner_digest = Sha256::digest(inner_input);
    let mut outer_input = Vec::with_capacity(64 + inner_digest.len());
    outer_input.extend_from_slice(&outer);
    outer_input.extend_from_slice(&inner_digest);
    hex_encode(&Sha256::digest(outer_input))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
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
    use super::{WhatsAppClient, WhatsAppConfig};
    use httpmock::prelude::*;
    use serde_json::json;

    fn config() -> WhatsAppConfig {
        WhatsAppConfig {
            access_token: "token".into(),
            phone_number_id: "phone".into(),
            verify_token: "verify".into(),
            app_secret: Some("secret".into()),
            graph_base_url: "https://graph.test".into(),
            allowed_users: ["1555".to_owned()].into_iter().collect(),
        }
    }

    #[test]
    fn verifies_challenge_signature_and_extracts_allowlisted_text() {
        let client = WhatsAppClient::new(config());
        assert_eq!(
            client.verify_challenge(Some("subscribe"), Some("verify"), Some("123")),
            Some("123".into())
        );
        let payload = json!({"entry": [{"changes": [{"value": {"messages": [{"from": "1555", "id": "wamid-1", "type": "text", "text": {"body": "hello"}}]}}]}]});
        assert_eq!(client.parse_messages(&payload)[0].text, "hello");
    }

    #[tokio::test]
    async fn sends_text_through_graph_api() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.method(POST).path("/phone/messages").json_body(json!({
                    "messaging_product": "whatsapp",
                    "to": "1555",
                    "type": "text",
                    "text": {"body": "hello"}
                }));
                then.status(200);
            })
            .await;
        let mut configuration = config();
        configuration.graph_base_url = server.base_url();
        WhatsAppClient::new(configuration)
            .send_text("1555", "hello")
            .await
            .unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn uploads_and_sends_native_document() {
        let server = MockServer::start_async().await;
        let upload = server
            .mock_async(|when, then| {
                when.method(POST).path("/phone/media");
                then.status(200).json_body(json!({"id": "media-1"}));
            })
            .await;
        let send = server
            .mock_async(|when, then| {
                when.method(POST).path("/phone/messages").json_body(json!({
                    "messaging_product": "whatsapp",
                    "to": "1555",
                    "type": "document",
                    "document": {"id": "media-1", "filename": "report.txt"}
                }));
                then.status(200);
            })
            .await;
        let mut configuration = config();
        configuration.graph_base_url = server.base_url();
        WhatsAppClient::new(configuration)
            .send_media("1555", b"report".to_vec(), "text/plain", "report.txt")
            .await
            .unwrap();
        upload.assert_async().await;
        send.assert_async().await;
    }

    #[test]
    fn rejects_bad_signature() {
        let client = WhatsAppClient::new(config());
        assert!(
            client
                .verify_signature(b"body", Some("sha256=bad"))
                .is_err()
        );
    }
}
