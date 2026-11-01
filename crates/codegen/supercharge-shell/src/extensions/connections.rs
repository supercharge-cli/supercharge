//! Owner-only plugin connection management over the existing authenticated ACP transport.
use crate::agent::MvpAgent;
use crate::plugin::connections::{self, Account};
use agent_client_protocol::{self as acp, Client};
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::HashMap;
thread_local! {
    static FLOWS: RefCell<HashMap<String, tokio::task::AbortHandle>> = RefCell::new(HashMap::new());
    static RESULTS: RefCell<HashMap<String, &'static str>> = RefCell::new(HashMap::new());
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    session_id: String,
    #[serde(default)]
    plugin: String,
    #[serde(default)]
    server: String,
    #[serde(default = "default_account")]
    account: String,
    #[serde(default)]
    id: String,
    client_id: Option<String>,
    scopes: Option<Vec<String>>,
    secret: Option<String>,
    secret_kind: Option<String>,
}
fn default_account() -> String {
    "default".into()
}

pub async fn handle(agent: &MvpAgent, args: &acp::ExtRequest) -> super::ExtResult {
    let req: Request = super::parse_params(args)?;
    let sid = acp::SessionId::new(req.session_id.clone());
    let handle = agent
        .get_session_handle(&sid)
        .ok_or_else(|| acp::Error::invalid_params().data("Session not found"))?;
    let result: anyhow::Result<serde_json::Value> = async {
        match args.method.as_ref() {
            "x.ai/connections/list" => connections::list(),
            "x.ai/connections/configure" => {
                agent.ensure_plugin_registry_async().await;
                let registry = agent.plugin_registry_snapshot().ok_or_else(|| anyhow::anyhow!("Plugin registry unavailable"))?;
                let plugins = registry.list();
                let plugin = plugins.iter().find(|p| p.name == req.plugin).ok_or_else(|| anyhow::anyhow!("Plugin not found"))?;
                let path = plugin.mcp_config_path.as_ref().ok_or_else(|| anyhow::anyhow!("Plugin has no file MCP configuration"))?;
                let config = crate::util::config::read_mcp_json(path).ok_or_else(|| anyhow::anyhow!("Invalid MCP configuration"))?;
                let config = config.mcp_servers.get(&req.server).ok_or_else(|| anyhow::anyhow!("Plugin server not found"))?;
                let crate::util::config::McpServerTransportConfig::StreamableHttp { url, .. } = &config.transport else { anyhow::bail!("Only HTTP connections require account setup"); };
                let url = url::Url::parse(url)?;
                anyhow::ensure!(url.scheme() == "https" && url.username().is_empty() && url.password().is_none(), "Account endpoint must use HTTPS without embedded credentials");
                let a = Account { plugin: req.plugin, server: req.server, origin: url.to_string(), account: req.account, client_id: req.client_id, scopes: req.scopes, secret_kind: req.secret_kind, ..Default::default() };
                tokio::task::spawn_blocking(move || connections::configure(a, req.secret)).await??;
                reload(agent, &sid).await
            }
            "x.ai/connections/select" => {
                tokio::task::spawn_blocking(move || connections::select(&req.id)).await??;
                reload(agent, &sid).await
            }
            "x.ai/connections/disconnect" => {
                let accounts = connections::list()?;
                let a = accounts["accounts"].as_array().and_then(|a| a.iter().find(|a| a["id"] == req.id)).ok_or_else(|| anyhow::anyhow!("Account not found"))?;
                let base = format!("{}--{}", a["server"].as_str().unwrap_or_default(), req.id.get(..16).unwrap_or_default());
                let plugin_name = a["plugin"].as_str().unwrap_or_default();
                let registry = agent.plugin_registry_snapshot();
                let portable = registry.as_ref().is_some_and(|r| r.list().iter().any(|p| p.name == plugin_name && p.mcp_config_path.as_ref().and_then(|p| p.file_name()).is_some_and(|n| n == "mcp.json")));
                let name = if portable { connections::namespaced(plugin_name, &base) } else { base };
                supercharge_mcp::oauth::cancel_consent(&name);
                FLOWS.with(|f| { if let Some(task) = f.borrow_mut().remove(&name) { task.abort(); } });
                let endpoint = url::Url::parse(a["origin"].as_str().unwrap_or_default())?;
                let id = req.id.clone();
                tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                    supercharge_mcp::credentials::McpCredentialStore::load_default()?.remove_and_save(&name, &endpoint)?;
                    connections::disconnect(&id)
                }).await??;
                let mut result = reload(agent, &sid).await?;
                result["providerRevocation"] = serde_json::json!("not_supported; revoke provider access separately");
                Ok(result)
            }
            "x.ai/connections/connect" => {
                let server = req.server;
                let running = FLOWS.with(|f| f.borrow().get(&server).is_some_and(|h| !h.is_finished()));
                if !running {
                    supercharge_mcp::oauth::prepare_consent(&server);
                    RESULTS.with(|r| { r.borrow_mut().remove(&server); });
                    let name = server.clone();
                    let gateway = agent.gateway.clone();
                    let task = tokio::task::spawn_local(async move {
                        let status = if handle.mcp_auth_trigger(name.clone()).await.is_ok() { "authenticated" } else { "failed" };
                        RESULTS.with(|r| { r.borrow_mut().insert(name.clone(), status); });
                        notify(&gateway, serde_json::json!({"server":name,"status":status})).await;
                    });
                    FLOWS.with(|f| { f.borrow_mut().insert(server, task.abort_handle()); });
                }
                Ok(serde_json::json!({"status":"pending"}))
            }
            "x.ai/connections/status" => {
                let status = RESULTS.with(|r| r.borrow().get(&req.server).copied()).unwrap_or_else(|| FLOWS.with(|f| if f.borrow().get(&req.server).is_some_and(|task| !task.is_finished()) { "pending" } else { "not_started" }));
                Ok(serde_json::json!({"status":status,"authorizationUrl":supercharge_mcp::oauth::pending_consent_url(&req.server)}))
            }
            "x.ai/connections/cancel" => {
                supercharge_mcp::oauth::cancel_consent(&req.server);
                FLOWS.with(|f| { if let Some(task) = f.borrow_mut().remove(&req.server) { task.abort(); } });
                RESULTS.with(|r| { r.borrow_mut().insert(req.server, "cancelled"); });
                Ok(serde_json::json!({"status":"cancelled"}))
            }
            _ => anyhow::bail!("Unknown connection operation"),
        }
    }.await;
    if result.is_ok()
        && !matches!(
            args.method.as_ref(),
            "x.ai/connections/list" | "x.ai/connections/status"
        )
    {
        notify(
            &agent.gateway,
            serde_json::json!({"operation":args.method.as_ref(),"status":"changed"}),
        )
        .await;
    }
    super::to_ext_response(result)
}
async fn notify(gateway: &supercharge_acp_lib::AcpAgentGatewaySender, value: serde_json::Value) {
    if let Ok(params) = serde_json::value::to_raw_value(&value) {
        let _ = gateway
            .ext_notification(acp::ExtNotification::new(
                "supercharge/connections/changed",
                params.into(),
            ))
            .await;
    }
}
async fn reload(agent: &MvpAgent, sid: &acp::SessionId) -> anyhow::Result<serde_json::Value> {
    let result = agent
        .execute_plugins_action(sid, supercharge_hooks_plugins_types::PluginsAction::Reload)
        .await
        .ok_or_else(|| anyhow::anyhow!("Session closed"))?;
    Ok(serde_json::to_value(result)?)
}
