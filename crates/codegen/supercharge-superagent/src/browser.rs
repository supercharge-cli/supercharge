use std::convert::Infallible;

use futures_util::{SinkExt, Stream, StreamExt};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::time::{Duration, timeout};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

pub const DEFAULT_STREAM_PORT: u16 = 9223;
pub const DEFAULT_MAX_FPS: u32 = 8;

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
    pub error: Option<String>,
}

impl StreamStatus {
    fn unavailable(port: u16, error: impl Into<String>) -> Self {
        Self {
            enabled: false,
            port,
            connected: false,
            screencasting: false,
            error: Some(error.into()),
        }
    }
}

pub fn stream_port() -> u16 {
    std::env::var("AGENT_BROWSER_STREAM_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|port: &u16| *port > 0)
        .unwrap_or(DEFAULT_STREAM_PORT)
}

async fn run_agent_browser(args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("agent-browser")
        .args(args)
        .output()
        .await
        .map_err(|error| format!("agent-browser unavailable: {error}"))
}

pub async fn read_stream_status(port: u16) -> StreamStatus {
    let output = match run_agent_browser(&["stream", "status", "--json"]).await {
        Ok(output) => output,
        Err(error) => return StreamStatus::unavailable(port, error),
    };
    if !output.status.success() {
        return StreamStatus::unavailable(
            port,
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .chars()
                .take(200)
                .collect::<String>(),
        );
    }
    serde_json::from_slice::<StreamStatus>(&output.stdout)
        .map(|mut status| {
            if status.port == 0 {
                status.port = port;
            }
            status
        })
        .unwrap_or_else(|_| {
            StreamStatus::unavailable(port, "agent-browser returned invalid status JSON")
        })
}

pub async fn ensure_stream(port: u16) -> StreamStatus {
    let current = read_stream_status(port).await;
    if current.enabled && (current.port == 0 || current.port == port) {
        return current;
    }
    let output = match run_agent_browser(&["stream", "enable", "--port", &port.to_string()]).await {
        Ok(output) => output,
        Err(error) => return StreamStatus::unavailable(port, error),
    };
    if !output.status.success() {
        return StreamStatus::unavailable(
            port,
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .chars()
                .take(200)
                .collect::<String>(),
        );
    }
    read_stream_status(port).await
}

type BrowserSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

pub async fn connect_browser_stream(port: u16) -> Result<(BrowserSocket, StreamStatus), String> {
    let status = ensure_stream(port).await;
    let url = Url::parse(&format!(
        "ws://127.0.0.1:{port}/?pacing=ack&maxFps={DEFAULT_MAX_FPS}"
    ))
    .map_err(|error| format!("invalid browser stream URL: {error}"))?;
    let (mut socket, _) = connect_async(url.as_str())
        .await
        .map_err(|error| format!("browser stream connection failed: {error}"))?;
    socket
        .send(Message::Text(
            format!(r#"{{"type":"config","pacing":"ack","maxFps":{DEFAULT_MAX_FPS}}}"#).into(),
        ))
        .await
        .map_err(|error| format!("browser stream configuration failed: {error}"))?;
    Ok((socket, status))
}

struct SseState {
    socket: BrowserSocket,
    first: Option<BytesEvent>,
}

type BytesEvent = Result<bytes::Bytes, Infallible>;

fn sse(value: &impl Serialize) -> BytesEvent {
    let encoded =
        serde_json::to_string(value).unwrap_or_else(|_| "{\"type\":\"error\"}".to_owned());
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

pub fn sse_stream(
    socket: BrowserSocket,
    initial: StreamStatus,
) -> impl Stream<Item = BytesEvent> + Send + 'static {
    let state = SseState {
        socket,
        first: Some(sse(&StatusEvent {
            kind: "status",
            connected: false,
            screencasting: false,
            port: initial.port,
            error: initial.error,
        })),
    };
    futures_util::stream::unfold(state, |mut state| async move {
        if let Some(first) = state.first.take() {
            return Some((first, state));
        }
        match timeout(Duration::from_secs(15), state.socket.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let raw = text.to_string();
                let parsed = serde_json::from_str::<serde_json::Value>(&raw).ok();
                let Some(object) = parsed.as_ref().and_then(serde_json::Value::as_object) else {
                    return Some((
                        Ok(bytes::Bytes::from(": malformed browser event\n\n")),
                        state,
                    ));
                };
                if object
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|kind| kind.starts_with("input_"))
                {
                    return Some((
                        Ok(bytes::Bytes::from(": ignored browser input event\n\n")),
                        state,
                    ));
                }
                if object.get("type").and_then(serde_json::Value::as_str) == Some("frame")
                    && let Some(seq) = object.get("seq").and_then(serde_json::Value::as_u64)
                {
                    let _ = state
                        .socket
                        .send(Message::Text(
                            format!(r#"{{"type":"ack","seq":{seq}}}"#).into(),
                        ))
                        .await;
                }
                let event = Ok(bytes::Bytes::from(format!("data: {raw}\n\n")));
                Some((event, state))
            }
            Ok(Some(Ok(Message::Ping(payload)))) => {
                let _ = state.socket.send(Message::Pong(payload)).await;
                Some((Ok(bytes::Bytes::from(": ping\n\n")), state))
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => None,
            Ok(Some(Err(error))) => Some((
                sse(&StatusEvent {
                    kind: "status",
                    connected: false,
                    screencasting: false,
                    port: stream_port(),
                    error: Some(error.to_string()),
                }),
                state,
            )),
            Err(_) => Some((Ok(bytes::Bytes::from(": ping\n\n")), state)),
            _ => None,
        }
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BrowserAction {
    Navigate {
        url: String,
    },
    Click {
        selector: String,
    },
    Fill {
        selector: String,
        text: String,
    },
    Type {
        selector: String,
        text: String,
    },
    Press {
        key: String,
    },
    Select {
        selector: String,
        value: String,
    },
    Scroll {
        direction: String,
        pixels: Option<u32>,
    },
    Wait {
        milliseconds: Option<u64>,
    },
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
const MAX_OUTPUT_CHARS: usize = 20_000;

fn validate_text(value: &str, label: &str, max: usize) -> Result<(), String> {
    if value.is_empty() || value.chars().count() > max || value.contains('\0') {
        return Err(format!(
            "{label} is empty, too long, or contains a null byte"
        ));
    }
    Ok(())
}

fn validate_selector(selector: &str) -> Result<(), String> {
    validate_text(selector, "selector", MAX_SELECTOR_CHARS)
}

fn validate_url(url: &str) -> Result<(), String> {
    validate_text(url, "url", MAX_URL_CHARS)?;
    let parsed = reqwest::Url::parse(url).map_err(|error| format!("invalid url: {error}"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(()),
        scheme => Err(format!("unsupported browser URL scheme: {scheme}")),
    }
}

fn cap_output(value: &[u8]) -> String {
    String::from_utf8_lossy(value)
        .chars()
        .take(MAX_OUTPUT_CHARS)
        .collect()
}

fn action_args(action: &BrowserAction) -> Result<(String, Vec<String>), String> {
    match action {
        BrowserAction::Navigate { url } => {
            validate_url(url)?;
            Ok(("navigate".to_owned(), vec!["open".to_owned(), url.clone()]))
        }
        BrowserAction::Click { selector } => {
            validate_selector(selector)?;
            Ok((
                "click".to_owned(),
                vec!["click".to_owned(), selector.clone()],
            ))
        }
        BrowserAction::Fill { selector, text } => {
            validate_selector(selector)?;
            validate_text(text, "text", MAX_TEXT_CHARS)?;
            Ok((
                "fill".to_owned(),
                vec!["fill".to_owned(), selector.clone(), text.clone()],
            ))
        }
        BrowserAction::Type { selector, text } => {
            validate_selector(selector)?;
            validate_text(text, "text", MAX_TEXT_CHARS)?;
            Ok((
                "type".to_owned(),
                vec!["type".to_owned(), selector.clone(), text.clone()],
            ))
        }
        BrowserAction::Press { key } => {
            validate_text(key, "key", 80)?;
            if key.chars().any(char::is_control) {
                return Err("key contains control characters".to_owned());
            }
            Ok(("press".to_owned(), vec!["press".to_owned(), key.clone()]))
        }
        BrowserAction::Select { selector, value } => {
            validate_selector(selector)?;
            validate_text(value, "value", MAX_TEXT_CHARS)?;
            Ok((
                "select".to_owned(),
                vec!["select".to_owned(), selector.clone(), value.clone()],
            ))
        }
        BrowserAction::Scroll { direction, pixels } => {
            let direction = direction.to_ascii_lowercase();
            if !matches!(direction.as_str(), "up" | "down" | "left" | "right") {
                return Err("scroll direction must be up, down, left, or right".to_owned());
            }
            let pixels = pixels.unwrap_or(700).clamp(1, 3_000);
            Ok((
                "scroll".to_owned(),
                vec!["scroll".to_owned(), direction, pixels.to_string()],
            ))
        }
        BrowserAction::Wait { milliseconds } => {
            let milliseconds = milliseconds.unwrap_or(500).clamp(0, 30_000);
            Ok((
                "wait".to_owned(),
                vec!["wait".to_owned(), milliseconds.to_string()],
            ))
        }
        BrowserAction::Back => Ok(("back".to_owned(), vec!["back".to_owned()])),
        BrowserAction::Forward => Ok(("forward".to_owned(), vec!["forward".to_owned()])),
        BrowserAction::Reload => Ok(("reload".to_owned(), vec!["reload".to_owned()])),
    }
}

pub async fn execute_action(action: BrowserAction) -> Result<BrowserActionResult, String> {
    let (name, args) = action_args(&action)?;
    let output = Command::new("agent-browser")
        .args(&args)
        .output()
        .await
        .map_err(|error| format!("agent-browser unavailable: {error}"))?;
    Ok(BrowserActionResult {
        ok: output.status.success(),
        action: name,
        stdout: cap_output(&output.stdout),
        stderr: cap_output(&output.stderr),
    })
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_MAX_FPS, DEFAULT_STREAM_PORT, StreamStatus, stream_port};

    #[test]
    fn defaults_match_the_computer_pane_contract() {
        assert_eq!(DEFAULT_STREAM_PORT, 9223);
        assert_eq!(DEFAULT_MAX_FPS, 8);
        assert_eq!(stream_port(), 9223);
        assert_eq!(
            StreamStatus::unavailable(9223, "offline").error.as_deref(),
            Some("offline")
        );
    }
}
