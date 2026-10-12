use std::path::Path;

use agent_client_protocol as acp;
use supercharge_acp_lib::{AcpAgentTx, acp_send};

use super::reducer::McpServer;
use supercharge_shell::util::config as cli_config;

pub(super) fn configured_servers(cwd: &Path) -> Vec<McpServer> {
    cli_config::load_mcp_servers(
        cwd,
        &supercharge_tools::types::compat::CompatConfig::default(),
    )
    .iter()
    .filter_map(|server| {
        let name = match server {
            acp::McpServer::Http(s) => &s.name,
            acp::McpServer::Sse(s) => &s.name,
            acp::McpServer::Stdio(s) => &s.name,
            _ => return None,
        };
        Some(McpServer {
            name: name.clone(),
            status: "pending".into(),
        })
    })
    .collect()
}

pub(super) fn decode_mcp_list(raw: &str) -> Option<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let result = value.get("result").unwrap_or(&value);
    result.get("servers")?.as_array()?;
    Some(result.clone())
}

pub(super) fn servers_from_list(value: serde_json::Value) -> Vec<McpServer> {
    let resolved = value
        .get("sessionMcpResolved")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    value
        .get("servers")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|server| {
            let name = server.get("name")?.as_str()?.to_string();
            let session = server.get("session").filter(|v| v.is_object());
            let status = if !resolved || session.is_none() {
                "pending"
            } else {
                let session = session?;
                if session
                    .get("blockedReason")
                    .and_then(|v| v.as_str())
                    .is_some()
                {
                    "failed"
                } else if session.get("setupRequired").and_then(|v| v.as_bool()) == Some(true)
                    || session.get("authRequired").and_then(|v| v.as_bool()) == Some(true)
                    || session.get("status").and_then(|v| v.as_str()) == Some("setuprequired")
                {
                    "needs-auth"
                } else if session.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
                    "disabled"
                } else {
                    match session.get("status").and_then(|v| v.as_str()) {
                        Some("ready") => "connected",
                        Some("unavailable") => "failed",
                        _ => "pending",
                    }
                }
            };
            Some(McpServer {
                name,
                status: status.into(),
            })
        })
        .collect()
}

pub(super) async fn resolve_mcp_servers_until(
    tx: &AcpAgentTx,
    session_id: &acp::SessionId,
    cwd: &Path,
    deadline: tokio::time::Instant,
) -> Vec<McpServer> {
    let mut fallback = configured_servers(cwd);
    let params = serde_json::json!({"sessionId": session_id.0.to_string()});
    let Ok(raw) = serde_json::value::to_raw_value(&params) else {
        return fallback;
    };
    let request = acp::ExtRequest::new("x.ai/mcp/list", raw.into());
    if let Ok(Ok(response)) = tokio::time::timeout_at(deadline, acp_send(request, tx)).await
        && let Some(list) = decode_mcp_list(response.0.get())
    {
        let listed = servers_from_list(list);
        for server in listed {
            if let Some(existing) = fallback.iter_mut().find(|s| s.name == server.name) {
                existing.status = server.status;
            } else {
                fallback.push(server);
            }
        }
    }
    fallback
}
