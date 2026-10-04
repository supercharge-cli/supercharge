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
    #[serde(flatten)]
    pub desktop: DesktopStatus,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DesktopStatus {
    #[serde(default)]
    pub desktop: bool,
    #[serde(default)]
    pub desktop_protocol: u8,
    #[serde(default)]
    pub handoff_protocol: u8,
    #[serde(default)]
    pub handoff_id: Option<String>,
    #[serde(default)]
    pub handoff_phase: String,
    #[serde(default)]
    pub expires_at: u64,
    #[serde(default)]
    pub help_requested: bool,
    #[serde(default)]
    pub bot_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
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
            desktop: DesktopStatus::default(),
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

#[derive(Clone, Debug)]
pub struct BotDesktop {
    pub bot_id: String,
    pub display: String,
    pub sidecar_port: u16,
    pub vnc_port: u16,
    pub runtime_dir: String,
}

pub async fn bot_desktop(bot_id: &str) -> Result<BotDesktop, String> {
    if bot_id.is_empty()
        || bot_id.len() > 80
        || !bot_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err("invalid bot ID".into());
    }
    if !is_e2b_runtime() {
        return Err("desktop requires E2B mode".into());
    }
    let script = "/opt/vectra/browser-runtime/start.sh";
    let output = tokio::time::timeout(
        Duration::from_secs(90),
        tokio::process::Command::new("bash")
            .arg(script)
            .env("SUPERAGENT_DESKTOP_BOT_ID", bot_id)
            .output(),
    )
    .await
    .map_err(|_| "desktop provisioning timed out".to_owned())?
    .map_err(|error| format!("desktop provisioning failed: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "desktop provisioning failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let slot = if bot_id == "main" {
        0
    } else {
        let path = format!("/tmp/supercharge-browser/bots/{bot_id}/slot");
        let slot = tokio::fs::read_to_string(path)
            .await
            .map_err(|error| error.to_string())?
            .trim()
            .parse::<u16>()
            .map_err(|_| "invalid desktop slot")?;
        if !(1..=300).contains(&slot) {
            return Err("invalid desktop slot".into());
        }
        slot
    };
    let target = target_for_slot(bot_id, slot);
    let status = sidecar_json_for(&target, reqwest::Method::GET, "/status", None).await?;
    if status
        .get("desktopBotId")
        .and_then(serde_json::Value::as_str)
        != Some(bot_id)
        || status.get("display").and_then(serde_json::Value::as_str) != Some(&target.display)
        || status.get("vncPort").and_then(serde_json::Value::as_u64)
            != Some(u64::from(target.vnc_port))
    {
        return Err("desktop sidecar identity mismatch".into());
    }
    Ok(target)
}

fn target_for_slot(bot_id: &str, slot: u16) -> BotDesktop {
    BotDesktop {
        bot_id: bot_id.to_owned(),
        display: format!(":{}", 99 + slot),
        sidecar_port: if slot == 0 {
            sidecar_port()
        } else {
            13000 + slot
        },
        vnc_port: if slot == 0 { 6080 } else { 11000 + slot },
        runtime_dir: if slot == 0 {
            "/tmp/supercharge-browser".into()
        } else {
            format!("/tmp/supercharge-browser/bots/{bot_id}")
        },
    }
}

async fn sidecar_json_for(
    target: &BotDesktop,
    method: reqwest::Method,
    path: &str,
    payload: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    sidecar_request(
        method,
        format!("http://127.0.0.1:{}{path}", target.sidecar_port),
        path,
        payload,
    )
    .await
}

async fn sidecar_json(
    method: reqwest::Method,
    path: &str,
    payload: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    if !is_e2b_runtime() {
        return Err("browser runtime is available only in E2B mode".to_owned());
    }
    sidecar_request(method, sidecar_url(path), path, payload).await
}

async fn sidecar_request(
    method: reqwest::Method,
    url: String,
    path: &str,
    payload: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let client = crate::http_client(|builder| {
        builder
            .timeout(Duration::from_secs(if path == "/status" { 2 } else { 20 }))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
    })
    .map_err(|error| error.to_string())?;
    let request = client
        .request(method, url)
        .header("x-vectra-browser-observer", "1");
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
    stream_status_result(
        sidecar_json(reqwest::Method::GET, "/status", None).await,
        port,
    )
}

pub async fn read_bot_stream_status(target: &BotDesktop) -> StreamStatus {
    let mut status = stream_status_result(
        sidecar_json_for(target, reqwest::Method::GET, "/status", None).await,
        target.sidecar_port,
    );
    status.desktop.bot_id = Some(target.bot_id.clone());
    status
}

fn stream_status_result(result: Result<serde_json::Value, String>, port: u16) -> StreamStatus {
    match result {
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
            desktop: serde_json::from_value(value).unwrap_or_default(),
        },
        Err(error) => StreamStatus::unavailable(port, error),
    }
}

pub async fn ensure_stream(port: u16) -> StreamStatus {
    read_stream_status(port).await
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
    desktop: bool,
}

/// Passive daemon-owned JPEG observer stream. noVNC interaction is never
/// exposed through this stream; it is available only through the handoff route.
pub fn sse_stream(
    initial: StreamStatus,
    target: BotDesktop,
) -> impl Stream<Item = BytesEvent> + Send + 'static {
    async_stream::stream! {
        yield sse(&StatusEvent {
            kind: "status",
            connected: initial.connected,
            screencasting: initial.screencasting,
            port: initial.port,
            error: initial.error,
            desktop: initial.desktop.desktop,
        });
        loop {
            match sidecar_json_for(&target, reqwest::Method::GET, "/screenshot", None).await {
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
                        port: target.sidecar_port,
                        error: Some(error),
                        desktop: false,
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

fn validate_text(value: &str, label: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() || value.chars().count() > max || value.contains('\0') {
        return Err(format!(
            "{label} is empty, too long, or contains a null byte"
        ));
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
            Ok((
                "navigate".to_owned(),
                serde_json::json!({"action":"navigate", "url":url}),
            ))
        }
        BrowserAction::Click { selector } => {
            validate_text(selector, "selector", MAX_SELECTOR_CHARS)?;
            Ok((
                "click".to_owned(),
                serde_json::json!({"action":"click", "selector":selector}),
            ))
        }
        BrowserAction::Fill { selector, text } | BrowserAction::Type { selector, text } => {
            validate_text(selector, "selector", MAX_SELECTOR_CHARS)?;
            validate_text(text, "text", MAX_TEXT_CHARS)?;
            let name = if matches!(action, BrowserAction::Fill { .. }) {
                "fill"
            } else {
                "type"
            };
            Ok((
                name.to_owned(),
                serde_json::json!({"action":name, "selector":selector, "text":text}),
            ))
        }
        BrowserAction::Press { key } => {
            validate_text(key, "key", 80)?;
            Ok((
                "press".to_owned(),
                serde_json::json!({"action":"press", "key":key}),
            ))
        }
        BrowserAction::Select { selector, value } => {
            validate_text(selector, "selector", MAX_SELECTOR_CHARS)?;
            validate_text(value, "value", MAX_TEXT_CHARS)?;
            Ok((
                "select".to_owned(),
                serde_json::json!({"action":"select", "selector":selector, "value":value}),
            ))
        }
        BrowserAction::Scroll { direction, pixels } => {
            let direction = direction.to_ascii_lowercase();
            if !matches!(direction.as_str(), "up" | "down" | "left" | "right") {
                return Err("scroll direction must be up, down, left, or right".to_owned());
            }
            Ok((
                "scroll".to_owned(),
                serde_json::json!({"action":"scroll", "direction":direction, "pixels":pixels.unwrap_or(700).clamp(1,3000)}),
            ))
        }
        BrowserAction::Wait { milliseconds } => Ok((
            "wait".to_owned(),
            serde_json::json!({"action":"wait", "milliseconds":milliseconds.unwrap_or(500).clamp(0,30000)}),
        )),
        BrowserAction::Back => Ok(("back".to_owned(), serde_json::json!({"action":"back"}))),
        BrowserAction::Forward => Ok((
            "forward".to_owned(),
            serde_json::json!({"action":"forward"}),
        )),
        BrowserAction::Reload => Ok(("reload".to_owned(), serde_json::json!({"action":"reload"}))),
    }
}

pub async fn execute_action(
    action: BrowserAction,
    handoff_id: &str,
    target: &BotDesktop,
) -> Result<BrowserActionResult, String> {
    let (name, mut payload) = action_payload(&action)?;
    payload["handoffId"] = serde_json::json!(handoff_id);
    let response = sidecar_json_for(target, reqwest::Method::POST, "/control", Some(payload)).await;
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

pub async fn handoff(
    start: bool,
    minutes: Option<u8>,
    handoff_id: &str,
    bot_id: Option<&str>,
    session_id: Option<&str>,
    target: &BotDesktop,
) -> Result<serde_json::Value, String> {
    let path = if start {
        "/handoff/start"
    } else {
        "/handoff/stop"
    };
    let payload = serde_json::json!({"minutes": minutes.unwrap_or(10).clamp(1, 30), "handoffId":handoff_id, "botId":bot_id, "sessionId":session_id});
    sidecar_json_for(target, reqwest::Method::POST, path, Some(payload)).await
}

pub async fn desktop_status() -> Result<DesktopStatus, String> {
    serde_json::from_value(sidecar_json(reqwest::Method::GET, "/status", None).await?)
        .map_err(|error| error.to_string())
}

pub async fn desktop_status_for(target: &BotDesktop) -> Result<DesktopStatus, String> {
    serde_json::from_value(sidecar_json_for(target, reqwest::Method::GET, "/status", None).await?)
        .map_err(|error| error.to_string())
}

pub async fn configure_desktop_environment(
    command: &mut tokio::process::Command,
    owner: &str,
    bot_id: &str,
) -> Result<(), String> {
    let target = bot_desktop(bot_id).await?;
    let status = desktop_status_for(&target).await?;
    if !status.desktop || status.desktop_protocol != 1 || status.handoff_protocol < 2 {
        return Err("desktop sidecar protocol unavailable".into());
    }
    command
        .env("VECTRA_BROWSER_OWNER", owner)
        .env("SUPERCHARGE_DESKTOP_PROTOCOL", "1")
        .env("DISPLAY", &target.display)
        .env("XAUTHORITY", format!("{}/Xauthority", target.runtime_dir))
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={}/bus", target.runtime_dir),
        )
        .env(
            "SUPERAGENT_BROWSER_SIDECAR_PORT",
            target.sidecar_port.to_string(),
        );
    Ok(())
}

pub async fn handoff_active() -> Result<bool, String> {
    let value = sidecar_json(reqwest::Method::GET, "/status", None).await?;
    Ok(value
        .get("handoff")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false))
}

pub async fn release_owner(owner: &str) -> Result<(), String> {
    sidecar_json(
        reqwest::Method::POST,
        "/lease/release",
        Some(serde_json::json!({"owner":owner})),
    )
    .await?;
    Ok(())
}

pub async fn reset_after_resume() -> Result<(), String> {
    sidecar_json(
        reqwest::Method::POST,
        "/lease/reset",
        Some(serde_json::json!({})),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        BrowserAction, DEFAULT_MAX_FPS, DEFAULT_SIDECAR_PORT, DEFAULT_STREAM_PORT, StreamStatus,
        action_payload, stream_port, target_for_slot,
    };

    #[test]
    fn defaults_match_the_computer_pane_contract() {
        assert_eq!(DEFAULT_STREAM_PORT, 9224);
        assert_eq!(DEFAULT_MAX_FPS, 8);
        assert_eq!(DEFAULT_SIDECAR_PORT, 9224);
        assert_eq!(stream_port(), 9224);
    }

    #[test]
    fn bot_slots_keep_desktops_and_ports_separate_from_main_and_each_other() {
        let main = target_for_slot("main", 0);
        let research = target_for_slot("research", 1);
        let design = target_for_slot("design", 300);
        assert_eq!(
            (main.display.as_str(), main.sidecar_port, main.vnc_port),
            (":99", 9224, 6080)
        );
        assert_eq!(
            (
                research.display.as_str(),
                research.sidecar_port,
                research.vnc_port
            ),
            (":100", 13001, 11001)
        );
        assert_ne!(design.display, research.display);
        assert_ne!(design.sidecar_port, research.sidecar_port);
        assert_ne!(design.vnc_port, research.vnc_port);
        assert!(research.runtime_dir.ends_with("/research"));
    }

    #[test]
    fn browser_actions_reject_invalid_urls() {
        assert!(
            action_payload(&BrowserAction::Navigate {
                url: "file:///etc/passwd".to_owned()
            })
            .is_err()
        );
        assert!(
            action_payload(&BrowserAction::Navigate {
                url: "https://example.com".to_owned()
            })
            .is_ok()
        );
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
