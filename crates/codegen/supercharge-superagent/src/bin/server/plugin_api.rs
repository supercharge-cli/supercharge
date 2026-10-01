use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{AppRuntime, AppState, ErrorResponse, bot_definition, internal_error};
use supercharge_superagent::plugin_control::{
    BotIdentity, PluginBridgeStatus, PluginControlResult, descriptor_to_acp, request as control,
};

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum PluginRequest {
    Status,
    Validate {
        manifest: Value,
    },
    Install {
        manifest: Value,
        request_id: String,
    },
    PluginAction {
        plugin_id: String,
        plugin_action: Value,
        request_id: String,
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

fn conversation(
    state: &AppState,
    bot_id: &str,
    session_id: &str,
) -> Result<super::DaemonSession, (StatusCode, Json<ErrorResponse>)> {
    let session = state
        .sessions
        .blocking_lock()
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

fn ext_result(value: Value) -> Result<Value, String> {
    if let Some(error) = value.get("error") {
        if !error.is_null() {
            return Err(error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
                .unwrap_or("Agent MCP request failed")
                .chars()
                .take(240)
                .collect());
        }
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
    let session = conversation(state, bot_id, session_id)?;
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
    runtime
        .cli
        .update_mcp_servers(vec![mcp])
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
    bot_definition(&state, &bot_id).await?;
    let result = match request {
        PluginRequest::Status => request_for(&state, &bot_id, "status", json!({})).await,
        PluginRequest::Validate { manifest } => {
            request_for(&state, &bot_id, "validate", json!({"manifest":manifest})).await
        }
        PluginRequest::Install {
            manifest,
            request_id,
        } => {
            request_for(
                &state,
                &bot_id,
                "install",
                json!({"manifest":manifest,"requestId":request_id}),
            )
            .await
        }
        PluginRequest::PluginAction {
            plugin_id,
            plugin_action,
            request_id,
        } => {
            request_for(
                &state,
                &bot_id,
                "pluginAction",
                json!({
                    "pluginId":plugin_id,
                    "pluginAction":plugin_action,
                    "requestId":request_id,
                }),
            )
            .await
        }
        PluginRequest::Apply {
            session_id,
            plugin_id,
            server_id,
            expected_revision,
        } => {
            let session = conversation(&state, &bot_id, &session_id)?;
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
            conversation(&state, &bot_id, &session_id)?;
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
            conversation(&state, &bot_id, &session_id)?;
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
            conversation(&state, &bot_id, &session_id)?;
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
                    runtime
                        .cli
                        .update_mcp_servers(Vec::new())
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
    Ok(Json(serde_json::to_value(result).map_err(internal_error)?))
}
