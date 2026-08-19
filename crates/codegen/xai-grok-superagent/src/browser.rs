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
