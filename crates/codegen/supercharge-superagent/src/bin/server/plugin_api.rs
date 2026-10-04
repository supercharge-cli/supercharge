use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{AppRuntime, AppState, ErrorResponse, bot_definition, internal_error};
use supercharge_superagent::plugin_control::{
    BotIdentity, PluginBridgeStatus, PluginControlResult, descriptor_to_acp, request as control,
};

#[derive(Debug, Deserialize, Serialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum PluginRequest {
    Status,
    Validate {
        manifest: Value,
    },
    Install {
        manifest: Value,
        request_id: String,
    },
    InstallCatalog {
        catalog_id: String,
        catalog_sha256: String,
        request_id: String,
    },
    PluginAction {
        plugin_id: String,
        plugin_action: Value,
        request_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        catalog_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        catalog_sha256: Option<String>,
    },
    Apply {
        session_id: String,
        plugin_id: String,
        server_id: String,
        expected_revision: Option<u64>,
    },
    Retry {
        session_id: String,
        expected_revision: u64,
    },
    Verify {
        session_id: String,
        expected_revision: u64,
        tool: String,
        #[serde(default)]
        arguments: Value,
        consent: bool,
    },
    Remove {
        session_id: String,
        expected_revision: u64,
    },
}

fn sandbox_id() -> Result<String, String> {
    for key in ["E2B_SANDBOX_ID", "SUPERAGENT_SANDBOX_ID"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return Ok(value.to_string());
            }
        }
    }
    for path in ["/run/e2b/.E2B_SANDBOX_ID", "/run/e2b/E2B_SANDBOX_ID"] {
        if let Ok(value) = std::fs::read_to_string(path) {
            let value = value.trim();
            if !value.is_empty() {
                return Ok(value.to_string());
            }
        }
    }
    Err("Sandbox generation identity is unavailable".into())
}

fn instance_id() -> Result<String, String> {
    std::env::var("SUPERAGENT_INSTANCE_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Instance identity is unavailable".into())
}

async fn request_for(
    state: &AppState,
    bot_id: &str,
    action: &str,
    body: Value,
) -> Result<PluginControlResult, String> {
    let bot = state
        .bot_catalog
        .lock()
        .await
        .get(bot_id)
        .ok_or("Bot not found or archived")?;
    control(
        &instance_id()?,
        &sandbox_id()?,
        BotIdentity {
            id: &bot.id,
            revision: bot.revision,
            default_folder: &bot.default_folder,
        },
        action,
        body,
    )
    .await
}

async fn conversation(
    state: &AppState,
    bot_id: &str,
    session_id: &str,
) -> Result<super::DaemonSession, (StatusCode, Json<ErrorResponse>)> {
    let session = state
        .sessions
        .lock()
        .await
        .get(session_id)
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Conversation not found".into(),
                }),
            )
        })?;
    if session.bot_id != bot_id {
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: "Conversation belongs to another bot".into(),
            }),
        ));
    }
    Ok(session)
}

fn merged_mcp_servers(
    mut servers: Vec<Value>,
    managed: Option<Value>,
    name: &str,
) -> Result<Vec<Value>, String> {
    let previous = servers
        .iter()
        .filter(|server| server.get("name").and_then(Value::as_str) == Some(name))
        .collect::<Vec<_>>();
    if previous.len() > 1
        || previous.iter().any(|server| {
            server
                .get("env")
                .and_then(Value::as_array)
                .is_none_or(|env| {
                    !env.iter().any(|entry| {
                        entry.get("name").and_then(Value::as_str)
                            == Some("SUPERCHARGE_PLUGIN_BRIDGE_BINDING")
                    })
                })
        })
    {
        return Err("Managed MCP name conflicts with another server".into());
    }
    servers.retain(|server| server.get("name").and_then(Value::as_str) != Some(name));
    if let Some(managed) = managed {
        servers.push(managed);
    }
    Ok(servers)
}

fn ext_result(value: Value) -> Result<Value, String> {
    if let Some(error) = value.get("error")
        && !error.is_null()
    {
        return Err(error
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| error.as_str())
            .unwrap_or("Agent MCP request failed")
            .chars()
            .take(240)
            .collect());
    }
    Ok(value.get("result").cloned().unwrap_or(value))
}

fn discover(value: Value, name: &str) -> Result<Vec<Value>, String> {
    let value = ext_result(value)?;
    let server = value
        .get("servers")
        .and_then(Value::as_array)
        .and_then(|servers| {
            servers
                .iter()
                .find(|server| server.get("name").and_then(Value::as_str) == Some(name))
        })
        .ok_or("Managed MCP was not discovered by the exact bot conversation")?;
    let session = server
        .get("session")
        .and_then(Value::as_object)
        .ok_or("Managed MCP list fell back outside the exact bot conversation")?;
    if session.get("enabled").and_then(Value::as_bool) != Some(true)
        || session.get("status").and_then(Value::as_str) != Some("ready")
    {
        return Err("Managed MCP is not ready in the exact bot conversation".into());
    }
    Ok(session
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

async fn runtime_for(
    state: &Arc<AppState>,
    session_id: &str,
) -> Result<Arc<AppRuntime>, (StatusCode, Json<ErrorResponse>)> {
    state
        .app_runtimes
        .lock()
        .await
        .get(session_id)
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::CONFLICT,
                Json(ErrorResponse {
                    error: "Open the selected bot conversation before applying managed MCP".into(),
                }),
            )
        })
}

async fn activate(
    state: &Arc<AppState>,
    bot_id: &str,
    session_id: &str,
    expected_revision: u64,
) -> Result<PluginBridgeStatus, (StatusCode, Json<ErrorResponse>)> {
    let session = conversation(state, bot_id, session_id).await?;
    let runtime = runtime_for(state, session_id).await?;
    if runtime.cli.is_prompting() {
        let marked = request_for(
            state,
            bot_id,
            "mark",
            json!({
                "sessionId":session_id,
                "expectedRevision":expected_revision,
                "status":"waiting",
            }),
        )
        .await
        .map_err(internal_error)?;
        return marked
            .binding
            .ok_or_else(|| internal_error("Plugin binding status missing"));
    }
    let target = request_for(
        state,
        bot_id,
        "activateBinding",
        json!({"sessionId":session_id,"expectedRevision":expected_revision}),
    )
    .await
    .map_err(internal_error)?;
    let descriptor = target
        .descriptor
        .as_ref()
        .ok_or_else(|| internal_error("Plugin bridge descriptor is unavailable"))?;
    let mcp = descriptor_to_acp(descriptor).map_err(internal_error)?;
    let servers = merged_mcp_servers(runtime.cli.mcp_servers(), Some(mcp), &descriptor.name)
        .map_err(internal_error)?;
    runtime
        .cli
        .update_mcp_servers(servers)
        .await
        .map_err(internal_error)?;
    let tools = discover(
        runtime.cli.list_mcp().await.map_err(internal_error)?,
        &descriptor.name,
    )
    .map_err(internal_error)?;
    let marked = request_for(
        state,
        bot_id,
        "mark",
        json!({
            "sessionId":session.id,
            "expectedRevision":expected_revision,
            "agentSessionId":runtime.cli.session_id,
            "status":"applied",
            "tools":tools,
        }),
    )
    .await
    .map_err(internal_error)?;
    marked
        .binding
        .ok_or_else(|| internal_error("Plugin binding status missing"))
}

pub(super) async fn restore_for_launch(
    state: &Arc<AppState>,
    bot_id: &str,
    session_id: &str,
    agent_session_id: &str,
) -> Result<Option<Value>, String> {
    if !supercharge_superagent::plugin_control::available() || state.api_token.is_none() {
        return Ok(None);
    }
    let status = request_for(state, bot_id, "status", json!({})).await?;
    let Some(binding) = status
        .bindings
        .as_ref()
        .and_then(|bindings| bindings.get(session_id))
    else {
        return Ok(None);
    };
    if matches!(
        binding.get("status").and_then(Value::as_str),
        Some("removed" | "removing")
    ) {
        return Ok(None);
    }
    if binding.get("agentSessionId").and_then(Value::as_str) != Some(agent_session_id) {
        return Err("Saved managed MCP belongs to a different agent session".into());
    }
    let target = request_for(
        state,
        bot_id,
        "activateBinding",
        json!({"sessionId":session_id,"expectedRevision":binding.get("revision")}),
    )
    .await?;
    let descriptor = target
        .descriptor
        .ok_or("Saved managed MCP descriptor is unavailable")?;
    descriptor_to_acp(&descriptor).map(Some)
}

pub(super) async fn reconcile_after_turn(state: &Arc<AppState>, bot_id: &str, session_id: &str) {
    if !supercharge_superagent::plugin_control::available() || state.api_token.is_none() {
        return;
    }
    let status = match request_for(state, bot_id, "status", json!({})).await {
        Ok(status) => status,
        Err(_) => return,
    };
    let Some(binding) = status
        .bindings
        .as_ref()
        .and_then(|bindings| bindings.get(session_id))
    else {
        return;
    };
    let Some(revision) = binding.get("revision").and_then(Value::as_u64) else {
        return;
    };
    match binding.get("status").and_then(Value::as_str) {
        Some("waiting") => {
            let _ = activate(state, bot_id, session_id, revision).await;
        }
        Some("removing") => {
            let runtime = state.app_runtimes.lock().await.get(session_id).cloned();
            if let Some(runtime) = runtime {
                if runtime.cli.is_prompting() {
                    return;
                }
                let servers = match merged_mcp_servers(
                    runtime.cli.mcp_servers(),
                    None,
                    binding
                        .get("serverName")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                ) {
                    Ok(servers) => servers,
                    Err(_) => return,
                };
                if runtime.cli.update_mcp_servers(servers).await.is_err() {
                    return;
                }
            }
            if request_for(
                state,
                bot_id,
                "revokeBinding",
                json!({"sessionId":session_id}),
            )
            .await
            .is_ok()
            {
                let _ = request_for(
                    state,
                    bot_id,
                    "finishRemove",
                    json!({"sessionId":session_id}),
                )
                .await;
            }
        }
        _ => {}
    }
}

pub(super) async fn plugin_api(
    State(state): State<Arc<AppState>>,
    Path((bot_id,)): Path<(String,)>,
    Json(request): Json<PluginRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    if !supercharge_superagent::browser::is_e2b_runtime() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Managed MCP is available only in Supercharge E2B".into(),
            }),
        ));
    }
    if state.api_token.is_none() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "Managed MCP requires authenticated sandbox control".into(),
            }),
        ));
    }
    if state
        .app_maintenance
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "Sandbox is pausing; retry after resume".into(),
            }),
        ));
    }
    bot_definition(&state, &bot_id).await?;
    let result = match request {
        request @ (PluginRequest::Status
        | PluginRequest::Validate { .. }
        | PluginRequest::Install { .. }
        | PluginRequest::InstallCatalog { .. }
        | PluginRequest::PluginAction { .. }) => {
            let body = serde_json::to_value(request).map_err(internal_error)?;
            let action = body["action"]
                .as_str()
                .ok_or_else(|| internal_error("Plugin control action missing"))?
                .to_owned();
            request_for(&state, &bot_id, &action, body).await
        }
        PluginRequest::Apply {
            session_id,
            plugin_id,
            server_id,
            expected_revision,
        } => {
            let session = conversation(&state, &bot_id, &session_id).await?;
            let runtime = runtime_for(&state, &session_id).await?;
            let target = request_for(
                &state,
                &bot_id,
                "stageBinding",
                json!({
                    "sessionId":session.id,
                    "agentSessionId":runtime.cli.session_id,
                    "pluginId":plugin_id,
                    "serverId":server_id,
                    "expectedRevision":expected_revision,
                }),
            )
            .await
            .map_err(internal_error)?;
            let binding = target
                .binding
                .ok_or_else(|| internal_error("Plugin binding status missing"))?;
            if runtime.cli.is_prompting() {
                Ok(PluginControlResult {
                    binding: Some(binding),
                    ..target
                })
            } else {
                let applied = activate(
                    &state,
                    &bot_id,
                    &session_id,
                    binding
                        .revision
                        .ok_or_else(|| internal_error("Plugin binding revision missing"))?,
                )
                .await?;
                Ok(PluginControlResult {
                    binding: Some(applied),
                    ..target
                })
            }
        }
        PluginRequest::Retry {
            session_id,
            expected_revision,
        } => {
            conversation(&state, &bot_id, &session_id).await?;
            let applied = activate(&state, &bot_id, &session_id, expected_revision).await?;
            Ok(PluginControlResult {
                binding: Some(applied),
                service: None,
                inventory: None,
                bindings: None,
                descriptor: None,
                agent_uid: None,
                plugin: None,
                valid: None,
                manifest: None,
            })
        }
        PluginRequest::Verify {
            session_id,
            expected_revision,
            tool,
            arguments,
            consent,
        } => {
            if !consent {
                return Err((
                    StatusCode::FORBIDDEN,
                    Json(ErrorResponse {
                        error: "Explicit consent is required for a real MCP verification call"
                            .into(),
                    }),
                ));
            }
            conversation(&state, &bot_id, &session_id).await?;
            let runtime = runtime_for(&state, &session_id).await?;
            if runtime.cli.is_prompting() {
                return Err((
                    StatusCode::CONFLICT,
                    Json(ErrorResponse {
                        error: "Wait for the bot turn to finish before verification".into(),
                    }),
                ));
            }
            let status = request_for(&state, &bot_id, "status", json!({}))
                .await
                .map_err(internal_error)?;
            let binding = status
                .bindings
                .as_ref()
                .and_then(|bindings| bindings.get(&session_id))
                .cloned()
                .ok_or_else(|| internal_error("Plugin binding not found"))?;
            if binding.get("revision").and_then(Value::as_u64) != Some(expected_revision)
                || binding.get("status").and_then(Value::as_str) != Some("applied")
                || !binding
                    .get("tools")
                    .and_then(Value::as_array)
                    .is_some_and(|tools| {
                        tools
                            .iter()
                            .any(|item| item.get("name").and_then(Value::as_str) == Some(&tool))
                    })
            {
                return Err((
                    StatusCode::CONFLICT,
                    Json(ErrorResponse {
                        error: "Managed MCP binding changed or tool was not discovered".into(),
                    }),
                ));
            }
            let server = binding
                .get("serverName")
                .and_then(Value::as_str)
                .ok_or_else(|| internal_error("Managed MCP server is unavailable"))?;
            let call = ext_result(
                runtime
                    .cli
                    .call_mcp(server, &tool, arguments)
                    .await
                    .map_err(internal_error)?,
            )
            .map_err(internal_error)?;
            if call.get("isError").and_then(Value::as_bool) == Some(true) {
                return Err(internal_error(
                    "Managed MCP verification call returned an error",
                ));
            }
            request_for(
                &state,
                &bot_id,
                "mark",
                json!({
                    "sessionId":session_id,
                    "expectedRevision":expected_revision,
                    "agentSessionId":runtime.cli.session_id,
                    "status":"ready",
                    "tools":binding.get("tools").cloned().unwrap_or_else(|| json!([])),
                }),
            )
            .await
        }
        PluginRequest::Remove {
            session_id,
            expected_revision,
        } => {
            conversation(&state, &bot_id, &session_id).await?;
            let begun = request_for(
                &state,
                &bot_id,
                "beginRemove",
                json!({"sessionId":session_id,"expectedRevision":expected_revision}),
            )
            .await
            .map_err(internal_error)?;
            let binding = begun
                .binding
                .ok_or_else(|| internal_error("Plugin binding status missing"))?;
            if let Ok(runtime) = runtime_for(&state, &session_id).await {
                if runtime.cli.is_prompting() {
                    Ok(PluginControlResult {
                        binding: Some(binding),
                        ..begun
                    })
                } else {
                    let servers = merged_mcp_servers(
                        runtime.cli.mcp_servers(),
                        None,
                        binding.server_name.as_deref().unwrap_or_default(),
                    )
                    .map_err(internal_error)?;
                    runtime
                        .cli
                        .update_mcp_servers(servers)
                        .await
                        .map_err(internal_error)?;
                    request_for(
                        &state,
                        &bot_id,
                        "revokeBinding",
                        json!({"sessionId":session_id}),
                    )
                    .await
                    .map_err(internal_error)?;
                    request_for(
                        &state,
                        &bot_id,
                        "finishRemove",
                        json!({"sessionId":session_id}),
                    )
                    .await
                }
            } else {
                request_for(
                    &state,
                    &bot_id,
                    "revokeBinding",
                    json!({"sessionId":session_id}),
                )
                .await
                .map_err(internal_error)?;
                request_for(
                    &state,
                    &bot_id,
                    "finishRemove",
                    json!({"sessionId":session_id}),
                )
                .await
            }
        }
    }
    .map_err(internal_error)?;
    let mut public = result;
    public.descriptor = None;
    public.agent_uid = None;
    Ok(Json(serde_json::to_value(public).map_err(internal_error)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_case_requests_preserve_controller_fields() {
        let digest = "a".repeat(64);
        let requests = [
            json!({"action":"status"}),
            json!({"action":"validate","manifest":{}}),
            json!({"action":"install","manifest":{},"requestId":"request-001"}),
            json!({"action":"installCatalog","catalogId":"vectra-echo","catalogSha256":digest,"requestId":"request-002"}),
            json!({"action":"pluginAction","pluginId":"vectra-echo","catalogId":"vectra-echo","catalogSha256":digest,"requestId":"request-003","pluginAction":{"action":"update","expectedRevision":2}}),
            json!({"action":"pluginAction","pluginId":"vectra-echo","requestId":"request-004","pluginAction":{"action":"enable","expectedRevision":2}}),
            json!({"action":"apply","sessionId":"session-1","pluginId":"vectra-echo","serverId":"echo","expectedRevision":null}),
            json!({"action":"retry","sessionId":"session-1","expectedRevision":1}),
            json!({"action":"verify","sessionId":"session-1","expectedRevision":1,"tool":"echo","arguments":{},"consent":true}),
            json!({"action":"remove","sessionId":"session-1","expectedRevision":1}),
        ];
        for body in requests {
            let request: PluginRequest = serde_json::from_value(body.clone()).unwrap();
            assert_eq!(serde_json::to_value(request).unwrap(), body);
        }
    }

    #[test]
    fn plugin_requests_reject_unknown_fields_and_missing_pins() {
        for body in [
            json!({"action":"installCatalog","catalogId":"vectra-echo","requestId":"request-001"}),
            json!({"action":"installCatalog","catalog_id":"vectra-echo","catalog_sha256":"a".repeat(64),"request_id":"request-001"}),
            json!({"action":"installCatalog","catalogId":"vectra-echo","catalogSha256":"a".repeat(64),"requestId":"request-001","source":{"url":"https://unreviewed.example/plugin.tar.gz"}}),
            json!({"action":"pluginAction","pluginId":"vectra-echo","requestId":"request-001","pluginAction":{"action":"update"},"catalogId":42}),
        ] {
            assert!(serde_json::from_value::<PluginRequest>(body).is_err());
        }
    }
}
