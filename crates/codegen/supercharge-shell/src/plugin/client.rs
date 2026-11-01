//! Small ACP management client shared by local CLI subprocess and remote owner connections.
use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

enum Transport {
    Stdio {
        child: tokio::process::Child,
        input: tokio::process::ChildStdin,
        output: tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    },
    Ws(
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ),
}
impl Transport {
    async fn send(&mut self, value: Value) -> Result<()> {
        let text = serde_json::to_string(&value)?;
        match self {
            Self::Stdio { input, .. } => {
                input.write_all(text.as_bytes()).await?;
                input.write_all(b"\n").await?;
                input.flush().await?;
            }
            Self::Ws(ws) => ws.send(Message::Text(text.into())).await?,
        }
        Ok(())
    }
    async fn receive(&mut self) -> Result<Value> {
        loop {
            let text = match self {
                Self::Stdio { output, .. } => output
                    .next_line()
                    .await?
                    .context("Agent closed connection")?,
                Self::Ws(ws) => match ws.next().await.context("Agent closed connection")?? {
                    Message::Text(s) => s.to_string(),
                    Message::Binary(b) => String::from_utf8(b.to_vec())?,
                    Message::Ping(p) => {
                        ws.send(Message::Pong(p)).await?;
                        continue;
                    }
                    Message::Close(_) => bail!("Agent closed connection"),
                    _ => continue,
                },
            };
            if let Ok(value) = serde_json::from_str(&text) {
                return Ok(value);
            }
        }
    }
    async fn rpc(&mut self, id: u64, method: &str, params: Value) -> Result<Value> {
        self.send(json!({"jsonrpc":"2.0", "id":id,"method":method,"params":params}))
            .await?;
        tokio::time::timeout(std::time::Duration::from_secs(180), async {
            loop {
                let value = self.receive().await?;
                if value.get("method").is_some() {
                    if let Some(id) = value.get("id") {
                        self.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Management client does not support reverse requests"}})).await?;
                    }
                    continue;
                }
                if value.get("id") == Some(&json!(id)) {
                    if let Some(error) = value.get("error") {
                        if error["code"] == -32601 { bail!("Update the selected agent to enable plugin management"); }
                        bail!("Agent request failed: {}", error["message"].as_str().unwrap_or("Unknown error"));
                    }
                    return Ok(value["result"].clone());
                }
            }
        }).await.context("Agent management request timed out")?
    }
}
/// `params` may contain secrets: never log or include it in error strings.
pub async fn request(
    agent: Option<&str>,
    cwd: &str,
    method: &str,
    mut params: Value,
) -> Result<Value> {
    let mut transport = if let Some(address) = agent {
        let url = url::Url::parse(address).context("Invalid agent URL")?;
        let loopback = url
            .host_str()
            .is_some_and(|h| matches!(h, "localhost" | "127.0.0.1" | "[::1]" | "::1"));
        anyhow::ensure!(
            url.scheme() == "wss" || (url.scheme() == "ws" && loopback),
            "Remote management requires wss://; ws:// is allowed only on loopback"
        );
        anyhow::ensure!(
            url.query().is_none()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none(),
            "Pass the agent key through SUPERCHARGE_AGENT_KEY, not the URL"
        );
        let key = std::env::var("SUPERCHARGE_AGENT_KEY")
            .context("Set SUPERCHARGE_AGENT_KEY for the selected remote agent")?;
        anyhow::ensure!(!key.is_empty(), "Agent key must not be empty");
        let mut req = address.into_client_request()?;
        req.headers_mut()
            .insert("Authorization", format!("Bearer {key}").parse()?);
        Transport::Ws(
            tokio_tungstenite::connect_async(req)
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "Could not connect to selected agent; check URL, TLS, and owner key"
                    )
                })?
                .0,
        )
    } else {
        let mut child = tokio::process::Command::new(std::env::current_exe()?)
            .args(["agent", "stdio"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let input = child.stdin.take().context("Agent stdin unavailable")?;
        let output =
            tokio::io::BufReader::new(child.stdout.take().context("Agent stdout unavailable")?)
                .lines();
        Transport::Stdio {
            child,
            input,
            output,
        }
    };
    let init = transport.rpc(1, "initialize", json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"supercharge-plugin-manager","version":supercharge_version::VERSION}})).await?;
    anyhow::ensure!(
        init["_meta"]["_supercharge/plugins"]["version"] == 1,
        "Update the selected agent to enable plugin management"
    );
    let session = transport
        .rpc(2, "session/new", json!({"cwd":cwd,"mcpServers":[]}))
        .await?;
    let sid = session["sessionId"]
        .as_str()
        .context("Agent did not create a management session")?;
    anyhow::ensure!(params.is_object(), "Input must be a JSON object");
    params["sessionId"] = json!(sid);
    let server = params.get("server").cloned().unwrap_or(Value::Null);
    let mut result = transport
        .rpc(3, &format!("_supercharge/{method}"), params)
        .await;
    if method == "connections/connect" && agent.is_none() && result.is_ok() {
        let mut previous = String::new();
        for index in 0..1200u64 {
            let progress = transport
                .rpc(
                    10 + index,
                    "_supercharge/connections/status",
                    json!({"sessionId":sid, "server":server}),
                )
                .await?;
            let state = progress.get("result").unwrap_or(&progress);
            if let Some(url) = state.get("authorizationUrl").and_then(Value::as_str) {
                if url != previous {
                    eprintln!("Open this sign-in link: {url}");
                    previous = url.to_owned();
                }
            }
            if state.get("status").and_then(Value::as_str) != Some("pending") {
                result = Ok(progress);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
    let _ = transport
        .rpc(4, "session/close", json!({"sessionId":sid}))
        .await;
    let result = result?;
    if let Some(error) = result.get("error") {
        anyhow::ensure!(
            error.is_null(),
            "Agent operation failed: {}",
            error.as_str().unwrap_or("Operation rejected")
        );
    }
    let value = result.get("result").cloned().unwrap_or(result);
    if let Some(status) = value.get("status").and_then(Value::as_str) {
        anyhow::ensure!(
            matches!(
                status,
                "success" | "pending" | "authenticated" | "cancelled"
            ),
            "{}",
            value["message"].as_str().unwrap_or("Operation failed")
        );
    }
    Ok(value)
}
