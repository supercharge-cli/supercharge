use std::convert::Infallible;

use futures_util::Stream;
use serde::{Deserialize, Serialize};
use tokio::time::Duration;

/// Browser frames are produced by the daemon from the localhost sidecar rather
/// than a separate public stream process.
pub const DEFAULT_STREAM_PORT: u16 = 9224;
pub const DEFAULT_MAX_FPS: u32 = 8;
pub const DEFAULT_SIDECAR_PORT: u16 = 9224;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreamStatus {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub screencasting: bool,
    #[serde(default)]
    pub handoff: bool,
    #[serde(default)]
    pub error: Option<String>,
}

impl StreamStatus {
    fn unavailable(port: u16, error: impl Into<String>) -> Self {
        Self {
            enabled: false,
            port,
            connected: false,
            screencasting: false,
            handoff: false,
            error: Some(error.into()),
        }
    }
}

pub fn is_e2b_runtime() -> bool {
    std::env::var("SUPERCHARGE_RUNTIME_MODE")
        .ok()
        .or_else(|| std::env::var("SUPERAGENT_RUNTIME_MODE").ok())
        .or_else(|| std::env::var("SUPERCHARGE_RUNTIME").ok())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("e2b"))
        || std::path::Path::new("/run/e2b/.E2B_SANDBOX_ID").exists()
        || std::path::Path::new("/run/e2b/E2B_SANDBOX_ID").exists()
        || std::env::var("E2B_SANDBOX_ID")
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
        || std::env::var("SUPERCHARGE_SANDBOX_ID")
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
}

pub fn stream_port() -> u16 {
    sidecar_port()
}

fn sidecar_port() -> u16 {
    std::env::var("SUPERAGENT_BROWSER_SIDECAR_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|port: &u16| *port > 0)
        .unwrap_or(DEFAULT_SIDECAR_PORT)
}

fn sidecar_url(path: &str) -> String {
    format!("http://127.0.0.1:{}{path}", sidecar_port())
}

async fn sidecar_json(
    method: reqwest::Method,
    path: &str,
    payload: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    if !is_e2b_runtime() {
        return Err("browser runtime is available only in E2B mode".to_owned());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    let request = client.request(method, sidecar_url(path)).header("x-vectra-browser-observer", "1");
    let response = match payload {
        Some(value) => request.json(&value).send().await,
        None => request.send().await,
    }
    .map_err(|error| format!("browser sidecar unavailable: {error}"))?;
    let status = response.status();
    let value = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("browser sidecar returned invalid JSON: {error}"))?;
    if !status.is_success() || value.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
        return Err(value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("browser sidecar request failed")
            .to_owned());
    }
    Ok(value)
}

pub async fn read_stream_status(port: u16) -> StreamStatus {
    match sidecar_json(reqwest::Method::GET, "/status", None).await {
        Ok(value) => StreamStatus {
            enabled: true,
            port,
            connected: true,
            screencasting: true,
            handoff: value
                .get("handoff")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            error: None,
        },
        Err(error) => StreamStatus::unavailable(port, error),
    }
}

pub async fn ensure_stream(port: u16) -> StreamStatus {
    read_stream_status(port).await
}

type BytesEvent = Result<bytes::Bytes, Infallible>;

fn sse(value: &impl Serialize) -> BytesEvent {
    let encoded = serde_json::to_string(value).unwrap_or_else(|_| "{\"type\":\"error\"}".to_owned());
    Ok(bytes::Bytes::from(format!("data: {encoded}\n\n")))
}

#[derive(Serialize)]
struct StatusEvent<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    connected: bool,
    screencasting: bool,
    port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Passive daemon-owned JPEG observer stream. noVNC interaction is never
/// exposed through this stream; it is available only through the handoff route.
pub fn sse_stream(initial: StreamStatus) -> impl Stream<Item = BytesEvent> + Send + 'static {
    async_stream::stream! {
        yield sse(&StatusEvent {
            kind: "status",
            connected: initial.connected,
            screencasting: initial.screencasting,
            port: initial.port,
            error: initial.error,
        });
        loop {
            match sidecar_json(reqwest::Method::GET, "/screenshot", None).await {
                Ok(value) => {
                    if let Some(jpeg) = value.get("jpeg").and_then(serde_json::Value::as_str) {
                        yield sse(&serde_json::json!({"type": "frame", "data": jpeg}));
                    }
                }
                Err(error) => {
                    yield sse(&StatusEvent {
                        kind: "status",
                        connected: false,
                        screencasting: false,
                        port: stream_port(),
                        error: Some(error),
                    });
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(1_000 / DEFAULT_MAX_FPS as u64)).await;
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BrowserAction {
    Navigate { url: String },
    Click { selector: String },
    Fill { selector: String, text: String },
    Type { selector: String, text: String },
    Press { key: String },
    Select { selector: String, value: String },
    Scroll { direction: String, pixels: Option<u32> },
    Wait { milliseconds: Option<u64> },
    Back,
    Forward,
    Reload,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowserActionResult {
    pub ok: bool,
    pub action: String,
    pub stdout: String,
    pub stderr: String,
}

const MAX_SELECTOR_CHARS: usize = 512;
const MAX_TEXT_CHARS: usize = 16_000;
const MAX_URL_CHARS: usize = 2_048;

fn validate_text(value: &str, label: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() || value.chars().count() > max || value.contains('\0') {
        return Err(format!("{label} is empty, too long, or contains a null byte"));
    }
    Ok(())
}

fn validate_url(url: &str) -> Result<(), String> {
    validate_text(url, "url", MAX_URL_CHARS)?;
    let parsed = reqwest::Url::parse(url).map_err(|error| format!("invalid url: {error}"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(()),
        scheme => Err(format!("unsupported browser URL scheme: {scheme}")),
    }
}

fn action_payload(action: &BrowserAction) -> Result<(String, serde_json::Value), String> {
    match action {
        BrowserAction::Navigate { url } => {
            validate_url(url)?;
            Ok(("navigate".to_owned(), serde_json::json!({"action":"navigate", "url":url})))
        }
        BrowserAction::Click { selector } => {
            validate_text(selector, "selector", MAX_SELECTOR_CHARS)?;
            Ok(("click".to_owned(), serde_json::json!({"action":"click", "selector":selector})))
        }
        BrowserAction::Fill { selector, text } | BrowserAction::Type { selector, text } => {
            validate_text(selector, "selector", MAX_SELECTOR_CHARS)?;
            validate_text(text, "text", MAX_TEXT_CHARS)?;
            let name = if matches!(action, BrowserAction::Fill { .. }) { "fill" } else { "type" };
            Ok((name.to_owned(), serde_json::json!({"action":name, "selector":selector, "text":text})))
        }
        BrowserAction::Press { key } => {
            validate_text(key, "key", 80)?;
            Ok(("press".to_owned(), serde_json::json!({"action":"press", "key":key})))
        }
        BrowserAction::Select { selector, value } => {
            validate_text(selector, "selector", MAX_SELECTOR_CHARS)?;
            validate_text(value, "value", MAX_TEXT_CHARS)?;
            Ok(("select".to_owned(), serde_json::json!({"action":"select", "selector":selector, "value":value})))
        }
        BrowserAction::Scroll { direction, pixels } => {
            let direction = direction.to_ascii_lowercase();
            if !matches!(direction.as_str(), "up" | "down" | "left" | "right") {
                return Err("scroll direction must be up, down, left, or right".to_owned());
            }
            Ok(("scroll".to_owned(), serde_json::json!({"action":"scroll", "direction":direction, "pixels":pixels.unwrap_or(700).clamp(1,3000)})))
        }
        BrowserAction::Wait { milliseconds } => Ok(("wait".to_owned(), serde_json::json!({"action":"wait", "milliseconds":milliseconds.unwrap_or(500).clamp(0,30000)}))),
        BrowserAction::Back => Ok(("back".to_owned(), serde_json::json!({"action":"back"}))),
        BrowserAction::Forward => Ok(("forward".to_owned(), serde_json::json!({"action":"forward"}))),
        BrowserAction::Reload => Ok(("reload".to_owned(), serde_json::json!({"action":"reload"}))),
    }
}

pub async fn execute_action(action: BrowserAction) -> Result<BrowserActionResult, String> {
    let (name, payload) = action_payload(&action)?;
    let response = sidecar_json(reqwest::Method::POST, "/control", Some(payload)).await;
    match response {
        Ok(value) => Ok(BrowserActionResult {
            ok: true,
            action: name,
            stdout: value.to_string(),
            stderr: String::new(),
        }),
        Err(error) => Ok(BrowserActionResult {
            ok: false,
            action: name,
            stdout: String::new(),
            stderr: error,
        }),
    }
}

pub async fn handoff(start: bool, minutes: Option<u8>) -> Result<serde_json::Value, String> {
    let path = if start { "/handoff/start" } else { "/handoff/stop" };
    let payload = start.then(|| serde_json::json!({"minutes": minutes.unwrap_or(10).clamp(1, 30)}));
    sidecar_json(reqwest::Method::POST, path, payload).await
}

pub async fn handoff_active() -> Result<bool, String> {
    let value = sidecar_json(reqwest::Method::GET, "/status", None).await?;
    Ok(value.get("handoff").and_then(serde_json::Value::as_bool).unwrap_or(false))
}

pub async fn release_owner(owner: &str) -> Result<(), String> {
    sidecar_json(reqwest::Method::POST, "/lease/release", Some(serde_json::json!({"owner":owner}))).await?;
    Ok(())
}

pub async fn reset_after_resume() -> Result<(), String> {
    sidecar_json(reqwest::Method::POST, "/lease/reset", Some(serde_json::json!({}))).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BrowserAction, DEFAULT_MAX_FPS, DEFAULT_SIDECAR_PORT, DEFAULT_STREAM_PORT, StreamStatus, action_payload, stream_port};

    #[test]
    fn defaults_match_the_computer_pane_contract() {
        assert_eq!(DEFAULT_STREAM_PORT, 9224);
        assert_eq!(DEFAULT_MAX_FPS, 8);
        assert_eq!(DEFAULT_SIDECAR_PORT, 9224);
        assert_eq!(stream_port(), 9224);
    }

    #[test]
    fn browser_actions_reject_invalid_urls() {
        assert!(action_payload(&BrowserAction::Navigate { url: "file:///etc/passwd".to_owned() }).is_err());
        assert!(action_payload(&BrowserAction::Navigate { url: "https://example.com".to_owned() }).is_ok());
    }

    #[test]
    fn stream_status_accepts_sidecar_handoff_state() {
        let status: StreamStatus = serde_json::from_value(serde_json::json!({
            "enabled": true,
            "connected": true,
            "screencasting": true,
            "handoff": true
        }))
        .expect("sidecar status must deserialize");
        assert!(status.handoff);
    }
}
