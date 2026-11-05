//! Minimal CDP client over the browser WebSocket.

use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::{connect_async, tungstenite::Message};

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
}

impl CdpSession {
    pub async fn connect(browser_ws: &str) -> Result<Self, CdpError> {
        let (ws, _) = connect_async(browser_ws).await?;
        let (write, read) = ws.split();
        Ok(Self {
            write,
            read,
            next_id: AtomicU64::new(1),
            session_id: None,
        })
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, CdpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(sid) = &self.session_id {
            msg.as_object_mut()
                .unwrap()
                .insert("sessionId".into(), json!(sid));
        }
        self.write.send(Message::Text(msg.to_string().into())).await?;
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
        self.session_id = attached
            .get("sessionId")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let _ = self.call("Page.enable", json!({})).await;
        let _ = self.call("Runtime.enable", json!({})).await;
        Ok(())
    }

    pub async fn navigate(&mut self, url: &str) -> Result<(), CdpError> {
        self.call("Page.navigate", json!({ "url": url })).await?;
        // Best-effort wait for load via Runtime.evaluate polling is enough for tests.
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
        Ok(result
            .pointer("/result/value")
            .cloned()
            .unwrap_or(Value::Null))
    }

    pub async fn page_text(&mut self) -> Result<String, CdpError> {
        let value = self
            .evaluate("document.body ? (document.body.innerText || document.body.textContent || '') : ''")
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

pub async fn browser_websocket_url(port: u16) -> Result<String, CdpError> {
    let url = format!("http://127.0.0.1:{port}/json/version");
    let value: Value = reqwest::Client::new()
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    value
        .get("webSocketDebuggerUrl")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| CdpError::Message("json/version missing webSocketDebuggerUrl".into()))
}
