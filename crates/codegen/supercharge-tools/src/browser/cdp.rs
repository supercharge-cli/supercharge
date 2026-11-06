//! Minimal CDP client over the browser WebSocket.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Bounds for one CDP connection, one JSON/version lookup, and one command round-trip.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CdpDeadlines {
    pub connect: Duration,
    pub call: Duration,
    pub discovery: Duration,
}

impl Default for CdpDeadlines {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            call: Duration::from_secs(20),
            discovery: Duration::from_secs(10),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CdpError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl CdpError {
    /// The page socket can no longer be reused. Callers should drop it.
    pub(crate) fn session_broken(&self) -> bool {
        match self {
            CdpError::Ws(_) => true,
            CdpError::Message(message) => {
                let lower = message.to_ascii_lowercase();
                lower.contains("timed out") || lower.contains("websocket closed")
            }
            CdpError::Http(_) | CdpError::Json(_) => false,
        }
    }
}

pub struct CdpSession {
    write: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    read: futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    next_id: AtomicU64,
    pub session_id: Option<String>,
    deadlines: CdpDeadlines,
}

impl CdpSession {
    pub async fn connect(browser_ws: &str) -> Result<Self, CdpError> {
        Self::connect_with_deadlines(browser_ws, CdpDeadlines::default()).await
    }

    pub(crate) async fn connect_with_deadlines(
        browser_ws: &str,
        deadlines: CdpDeadlines,
    ) -> Result<Self, CdpError> {
        let connect = tokio::time::timeout(deadlines.connect, connect_async(browser_ws))
            .await
            .map_err(|_| {
                CdpError::Message(format!(
                    "CDP websocket connect timed out after {}ms",
                    deadlines.connect.as_millis()
                ))
            })??;
        let (ws, _) = connect;
        let (write, read) = ws.split();
        Ok(Self {
            write,
            read,
            next_id: AtomicU64::new(1),
            session_id: None,
            deadlines,
        })
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, CdpError> {
        let budget = self.deadlines.call;
        let label = method.to_string();
        let result = tokio::time::timeout(budget, self.call_unlimited(method, params)).await;
        result.map_err(|_| {
            CdpError::Message(format!(
                "CDP call {label} timed out after {}ms",
                budget.as_millis()
            ))
        })?
    }

    async fn call_unlimited(&mut self, method: &str, params: Value) -> Result<Value, CdpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(sid) = &self.session_id {
            msg.as_object_mut()
                .unwrap()
                .insert("sessionId".into(), json!(sid));
        }
        self.write
            .send(Message::Text(msg.to_string().into()))
            .await?;
        loop {
            let Some(frame) = self.read.next().await else {
                return Err(CdpError::Message("CDP websocket closed".into()));
            };
            let frame = frame?;
            let Message::Text(text) = frame else {
                continue;
            };
            let value: Value = serde_json::from_str(&text)?;
            if value.get("id").and_then(|v| v.as_u64()) != Some(id) {
                continue;
            }
            if let Some(err) = value.get("error") {
                return Err(CdpError::Message(err.to_string()));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    pub async fn attach_page(&mut self, url: &str) -> Result<(), CdpError> {
        let created = self
            .call("Target.createTarget", json!({ "url": url }))
            .await?;
        let target_id = created
            .get("targetId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CdpError::Message("createTarget missing targetId".into()))?
            .to_string();
        let attached = self
            .call(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
            )
            .await?;
        let session_id = attached
            .get("sessionId")
            .and_then(|v| v.as_str())
            .filter(|sid| !sid.is_empty())
            .ok_or_else(|| CdpError::Message("attachToTarget missing sessionId".into()))?
            .to_string();
        self.session_id = Some(session_id);
        self.call("Page.enable", json!({})).await?;
        self.call("Runtime.enable", json!({})).await?;
        Ok(())
    }

    pub async fn navigate(&mut self, url: &str) -> Result<(), CdpError> {
        let result = self.call("Page.navigate", json!({ "url": url })).await?;
        if let Some(error_text) = result.get("errorText").and_then(|v| v.as_str()) {
            if !error_text.is_empty() {
                return Err(CdpError::Message(format!(
                    "Page.navigate failed: {error_text}"
                )));
            }
        }
        Ok(())
    }

    pub async fn evaluate(&mut self, expression: &str) -> Result<Value, CdpError> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        if let Some(details) = result
            .get("exceptionDetails")
            .filter(|value| !value.is_null())
        {
            return Err(CdpError::Message(format!(
                "JavaScript exception: {}",
                js_exception_message(details)
            )));
        }
        Ok(result
            .pointer("/result/value")
            .cloned()
            .unwrap_or(Value::Null))
    }

    pub async fn page_text(&mut self) -> Result<String, CdpError> {
        let value = self
            .evaluate(
                "document.body ? (document.body.innerText || document.body.textContent || '') : ''",
            )
            .await?;
        Ok(value.as_str().unwrap_or("").to_string())
    }

    pub async fn click_selector(&mut self, selector: &str) -> Result<(), CdpError> {
        let expr = format!(
            r#"(function(){{ const el=document.querySelector({sel}); if(!el) throw new Error('not found'); el.click(); return true; }})()"#,
            sel = serde_json::to_string(selector).unwrap()
        );
        self.evaluate(&expr).await?;
        Ok(())
    }

    pub async fn type_selector(&mut self, selector: &str, text: &str) -> Result<(), CdpError> {
        let expr = format!(
            r#"(function(){{ const el=document.querySelector({sel}); if(!el) throw new Error('not found'); el.focus(); el.value={text}; el.dispatchEvent(new Event('input',{{bubbles:true}})); return true; }})()"#,
            sel = serde_json::to_string(selector).unwrap(),
            text = serde_json::to_string(text).unwrap()
        );
        self.evaluate(&expr).await?;
        Ok(())
    }

    pub async fn screenshot_png(&mut self) -> Result<Vec<u8>, CdpError> {
        let result = self
            .call(
                "Page.captureScreenshot",
                json!({ "format": "png", "fromSurface": true }),
            )
            .await?;
        let b64 = result
            .get("data")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CdpError::Message("screenshot missing data".into()))?;
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| CdpError::Message(e.to_string()))
    }
}

fn js_exception_message(details: &Value) -> String {
    let text = details
        .get("text")
        .and_then(|value| value.as_str())
        .unwrap_or("JavaScript exception");
    let description = details
        .pointer("/exception/description")
        .and_then(|value| value.as_str())
        .or_else(|| {
            details
                .pointer("/exception/value")
                .and_then(|value| value.as_str())
        });
    match description {
        Some(description) if !description.is_empty() => format!("{text}: {description}"),
        _ => text.to_string(),
    }
}

pub async fn browser_websocket_url(port: u16) -> Result<String, CdpError> {
    browser_websocket_url_within(port, CdpDeadlines::default().discovery).await
}

async fn browser_websocket_url_within(port: u16, budget: Duration) -> Result<String, CdpError> {
    let url = format!("http://127.0.0.1:{port}/json/version");
    let fetch = async {
        let client = reqwest::Client::builder()
            .timeout(budget)
            .connect_timeout(budget)
            .build()
            .map_err(|err| CdpError::Message(format!("CDP discovery client: {err}")))?;
        let response = client.get(&url).send().await.map_err(|err| {
            if err.is_timeout() {
                CdpError::Message(format!(
                    "CDP discovery timed out after {}ms",
                    budget.as_millis()
                ))
            } else {
                CdpError::Http(err)
            }
        })?;
        let value: Value = response.error_for_status()?.json().await?;
        value
            .get("webSocketDebuggerUrl")
            .and_then(|entry| entry.as_str())
            .map(str::to_string)
            .ok_or_else(|| CdpError::Message("json/version missing webSocketDebuggerUrl".into()))
    };
    tokio::time::timeout(budget, fetch).await.map_err(|_| {
        CdpError::Message(format!(
            "CDP discovery timed out after {}ms",
            budget.as_millis()
        ))
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    fn short_deadlines() -> CdpDeadlines {
        CdpDeadlines {
            connect: Duration::from_millis(80),
            call: Duration::from_millis(80),
            discovery: Duration::from_millis(80),
        }
    }

    async fn scripted_ws(responses: Vec<Value>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut responses = responses.into_iter();
            while let Some(frame) = ws.next().await {
                let Ok(frame) = frame else { break };
                let Message::Text(text) = frame else { continue };
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    break;
                };
                let id = value.get("id").cloned().unwrap_or(Value::Null);
                let Some(mut body) = responses.next() else {
                    std::future::pending::<()>().await;
                    return;
                };
                if let Some(object) = body.as_object_mut() {
                    object.insert("id".into(), id);
                }
                if ws
                    .send(Message::Text(body.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        format!("ws://127.0.0.1:{port}")
    }

    #[test]
    fn navigate_failure_text_does_not_mark_the_socket_broken() {
        let navigate = CdpError::Message("Page.navigate failed: net::ERR_FAILED".into());
        assert!(!navigate.session_broken());
        let timed_out = CdpError::Message("CDP call Runtime.evaluate timed out after 80ms".into());
        assert!(timed_out.session_broken());
        let closed = CdpError::Message("CDP websocket closed".into());
        assert!(closed.session_broken());
    }

    #[tokio::test]
    async fn evaluate_returns_value_and_surfaces_exception_details() {
        let url = scripted_ws(vec![json!({
            "result": { "result": { "type": "number", "value": 7 } }
        })])
        .await;
        let mut session = CdpSession::connect_with_deadlines(&url, short_deadlines())
            .await
            .unwrap();
        let value = session.evaluate("1+6").await.unwrap();
        assert_eq!(value, json!(7));

        let url = scripted_ws(vec![json!({
            "result": {
                "result": { "type": "object", "subtype": "error" },
                "exceptionDetails": {
                    "text": "Uncaught",
                    "exception": { "description": "Error: not found" }
                }
            }
        })])
        .await;
        let mut session = CdpSession::connect_with_deadlines(&url, short_deadlines())
            .await
            .unwrap();
        let err = session
            .evaluate("document.querySelector('#missing')")
            .await
            .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("not found"), "{text}");
        assert!(text.contains("JavaScript exception"), "{text}");
        assert!(!err.session_broken());
    }

    #[tokio::test]
    async fn navigate_error_text_and_missing_session_id_are_errors() {
        let url = scripted_ws(vec![json!({
            "result": { "frameId": "f1", "errorText": "net::ERR_NAME_NOT_RESOLVED" }
        })])
        .await;
        let mut session = CdpSession::connect_with_deadlines(&url, short_deadlines())
            .await
            .unwrap();
        let err = session
            .navigate("http://missing.invalid")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("net::ERR_NAME_NOT_RESOLVED"),
            "{err}"
        );
        assert!(!err.session_broken());

        let url = scripted_ws(vec![
            json!({ "result": { "targetId": "target-1" } }),
            json!({ "result": {} }),
        ])
        .await;
        let mut session = CdpSession::connect_with_deadlines(&url, short_deadlines())
            .await
            .unwrap();
        let err = session.attach_page("about:blank").await.unwrap_err();
        assert!(err.to_string().contains("missing sessionId"), "{err}");
        assert!(session.session_id.is_none());
    }

    #[tokio::test]
    async fn call_connect_and_discovery_deadlines_fire_without_a_peer_reply() {
        let url = scripted_ws(Vec::new()).await;
        let mut session = CdpSession::connect_with_deadlines(&url, short_deadlines())
            .await
            .unwrap();
        let started = std::time::Instant::now();
        let err = session
            .call("Runtime.evaluate", json!({}))
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_millis(500), "call hung");
        assert!(err.to_string().contains("timed out"), "{err}");
        assert!(err.session_broken());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let started = std::time::Instant::now();
        let err = match CdpSession::connect_with_deadlines(
            &format!("ws://127.0.0.1:{port}"),
            short_deadlines(),
        )
        .await
        {
            Ok(_) => panic!("connect should time out"),
            Err(err) => err,
        };
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "connect hung"
        );
        assert!(err.to_string().contains("timed out"), "{err}");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let started = std::time::Instant::now();
        let err = browser_websocket_url_within(port, Duration::from_millis(80))
            .await
            .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "discovery hung"
        );
        assert!(
            err.to_string().to_lowercase().contains("timed out"),
            "{err}"
        );
    }
}
