use std::{env, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    discord::{DiscordClient, DiscordConfig},
    email::{EmailClient, EmailConfig},
    matrix::{MatrixClient, MatrixConfig},
    signal::{SignalClient, SignalConfig},
    slack::{SlackClient, SlackConfig},
    telegram::{TelegramClient, TelegramConfig},
    webhook::WebhookClient,
    whatsapp::{WhatsAppClient, WhatsAppConfig},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeliveryTarget {
    pub platform: String,
    pub recipient: String,
    pub thread: Option<String>,
}

impl DeliveryTarget {
    pub fn new(platform: impl Into<String>, recipient: impl Into<String>) -> Self {
        Self {
            platform: platform.into().to_ascii_lowercase(),
            recipient: recipient.into(),
            thread: None,
        }
    }

    pub fn session_channel(&self) -> String {
        match &self.thread {
            Some(thread) => format!("{}:{}:{}", self.platform, self.recipient, thread),
            None => format!("{}:{}", self.platform, self.recipient),
        }
    }
}

impl FromStr for DeliveryTarget {
    type Err = DeliveryError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut parts = value.splitn(3, ':');
        let platform = parts.next().unwrap_or_default().trim();
        let recipient = parts.next().unwrap_or_default().trim();
        let thread = parts
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if platform.is_empty() || recipient.is_empty() {
            return Err(DeliveryError::InvalidTarget(
                "target must be platform:recipient[:thread]".to_owned(),
            ));
        }
        if !matches!(
            platform,
            "telegram"
                | "discord"
                | "slack"
                | "whatsapp"
                | "signal"
                | "matrix"
                | "email"
                | "webhook"
        ) {
            return Err(DeliveryError::InvalidTarget(format!(
                "unsupported delivery platform: {platform}"
            )));
        }
        Ok(Self {
            platform: platform.to_owned(),
            recipient: recipient.to_owned(),
            thread: thread.map(str::to_owned),
        })
    }
}

#[derive(Debug, Error)]
pub enum DeliveryError {
    #[error("invalid delivery target: {0}")]
    InvalidTarget(String),
    #[error("delivery configuration failed: {0}")]
    Configuration(String),
    #[error("delivery failed: {0}")]
    Failed(String),
}

pub async fn dispatch_text(target: &DeliveryTarget, text: &str) -> Result<(), DeliveryError> {
    if text.trim().is_empty() {
        return Ok(());
    }
    match target.platform.as_str() {
        "telegram" => {
            let chat_id = target.recipient.parse::<i64>().map_err(|_| {
                DeliveryError::InvalidTarget("Telegram recipient must be numeric".to_owned())
            })?;
            TelegramClient::new(
                TelegramConfig::from_env()
                    .map_err(|error| DeliveryError::Configuration(error.to_string()))?,
            )
            .send_message(chat_id, text)
            .await
            .map_err(|error| DeliveryError::Failed(error.to_string()))
        }
        "discord" => DiscordClient::new(
            DiscordConfig::from_env()
                .map_err(|error| DeliveryError::Configuration(error.to_string()))?,
        )
        .send_message(&target.recipient, text)
        .await
        .map_err(|error| DeliveryError::Failed(error.to_string())),
        "slack" => SlackClient::new(
            SlackConfig::from_env()
                .map_err(|error| DeliveryError::Configuration(error.to_string()))?,
        )
        .send_message(&target.recipient, text, target.thread.as_deref())
        .await
        .map_err(|error| DeliveryError::Failed(error.to_string())),
        "whatsapp" => WhatsAppClient::new(
            WhatsAppConfig::from_env()
                .map_err(|error| DeliveryError::Configuration(error.to_string()))?,
        )
        .send_text(&target.recipient, text)
        .await
        .map_err(|error| DeliveryError::Failed(error.to_string())),
        "signal" => SignalClient::new(
            SignalConfig::from_env()
                .map_err(|error| DeliveryError::Configuration(error.to_string()))?,
        )
        .send_message(&target.recipient, text)
        .await
        .map_err(|error| DeliveryError::Failed(error.to_string())),
        "matrix" => MatrixClient::new(
            MatrixConfig::from_env()
                .map_err(|error| DeliveryError::Configuration(error.to_string()))?,
        )
        .send_message(&target.recipient, text)
        .await
        .map_err(|error| DeliveryError::Failed(error.to_string())),
        "email" => {
            let config = EmailConfig::from_env()
                .map_err(|error| DeliveryError::Configuration(error.to_string()))?;
            let subject =
                env::var("EMAIL_DELIVERY_SUBJECT").unwrap_or_else(|_| "Agent result".to_owned());
            EmailClient::new()
                .send_text(&config, &target.recipient, &subject, text)
                .await
                .map_err(|error| DeliveryError::Failed(error.to_string()))
        }
        "webhook" => {
            let url = env::var("GENERIC_WEBHOOK_URL").map_err(|_| {
                DeliveryError::Configuration("GENERIC_WEBHOOK_URL is required".to_owned())
            })?;
            let secret = env::var("GENERIC_WEBHOOK_SECRET").ok();
            WebhookClient::new()
                .deliver_json(
                    &url,
                    secret.as_deref(),
                    &serde_json::json!({"text": text, "target": target.session_channel()}),
                )
                .await
                .map_err(|error| DeliveryError::Failed(error.to_string()))
        }
        _ => Err(DeliveryError::InvalidTarget(format!(
            "unsupported delivery platform: {}",
            target.platform
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::DeliveryTarget;
    use std::str::FromStr;

    #[test]
    fn parses_threaded_and_plain_targets() {
        let threaded = DeliveryTarget::from_str("slack:C123:171234.5").unwrap();
        assert_eq!(threaded.session_channel(), "slack:C123:171234.5");
        let plain = DeliveryTarget::from_str("telegram:42").unwrap();
        assert_eq!(plain.session_channel(), "telegram:42");
        assert!(DeliveryTarget::from_str("unknown:1").is_err());
    }
}
