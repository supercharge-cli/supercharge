use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use agent_client_protocol as acp;
use supercharge_acp_lib::AcpAgentMessage;

use super::{decode_mcp_list, resolve_mcp_servers_until, servers_from_list};

fn statuses(
    servers: &[crate::headless::reducer::McpServer],
) -> std::collections::HashMap<&str, &str> {
    servers
        .iter()
        .map(|s| (s.name.as_str(), s.status.as_str()))
        .collect()
}

#[test]
fn statuses_are_honest_before_and_after_resolution() {
    let raw = serde_json::json!({"result": {"sessionMcpResolved": true, "servers": [
        {"name": "ready", "session": {"enabled": true, "status": "ready"}},
        {"name": "auth", "session": {"enabled": true, "authRequired": true}},
        {"name": "setup", "session": {"enabled": true, "setupRequired": true}},
        {"name": "disabled", "session": {"enabled": false}},
        {"name": "failed", "session": {"enabled": true, "status": "unavailable"}},
        {"name": "blocked", "session": {"enabled": false, "blockedReason": "policy"}},
        {"name": "boot", "session": {"enabled": true, "status": "initializing"}}
    ]}});
    let servers = servers_from_list(decode_mcp_list(&raw.to_string()).unwrap());
    let mapped = statuses(&servers);
    for (name, expected) in [
        ("ready", "connected"),
        ("auth", "needs-auth"),
        ("setup", "needs-auth"),
        ("disabled", "disabled"),
        ("failed", "failed"),
        ("blocked", "failed"),
        ("boot", "pending"),
    ] {
        assert_eq!(mapped.get(name).copied(), Some(expected), "{name}");
    }
    let unresolved = serde_json::json!({"sessionMcpResolved": false, "servers": [
        {"name": "unclaimed", "session": {"enabled": false}},
        {"name": "gone", "session": {"enabled": true, "status": "unavailable"}}
    ]});
    let servers = servers_from_list(decode_mcp_list(&unresolved.to_string()).unwrap());
    let mapped = statuses(&servers);
    assert_eq!(mapped.get("unclaimed").copied(), Some("pending"));
    assert_eq!(mapped.get("gone").copied(), Some("pending"));
}

#[tokio::test]
async fn snapshot_is_single_and_bounded_with_pending_fallback() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Some(AcpAgentMessage::ExtMethod(req)) = rx.recv().await {
            assert_eq!(req.request.method.as_ref(), "x.ai/mcp/list");
            count.fetch_add(1, Ordering::SeqCst);
            held.push(req.response_tx);
        }
    });
    let cwd = tempfile::tempdir().unwrap();
    let dir = cwd.path().join(".supercharge");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(
        dir.join("config.toml"),
        "[mcp_servers.configured]\ncommand = \"false\"\n",
    )
    .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(80);
    let result =
        resolve_mcp_servers_until(&tx, &acp::SessionId::new("sess-1"), cwd.path(), deadline).await;
    assert_eq!(
        statuses(&result).get("configured").copied(),
        Some("pending")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ready_snapshot_does_not_poll_again() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    tokio::spawn(async move {
        while let Some(AcpAgentMessage::ExtMethod(req)) = rx.recv().await {
            count.fetch_add(1, Ordering::SeqCst);
            let body = serde_json::json!({"result": {"sessionMcpResolved": true, "servers": [
                {"name": "live", "session": {"enabled": true, "status": "ready"}}]}});
            let raw = serde_json::value::to_raw_value(&body).unwrap();
            let _ = req
                .response_tx
                .send(Ok(acp::ExtResponse::new(Arc::from(raw))));
        }
    });
    let result = resolve_mcp_servers_until(
        &tx,
        &acp::SessionId::new("sess-1"),
        Path::new("."),
        tokio::time::Instant::now() + Duration::from_secs(1),
    )
    .await;
    assert_eq!(statuses(&result).get("live").copied(), Some("connected"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
