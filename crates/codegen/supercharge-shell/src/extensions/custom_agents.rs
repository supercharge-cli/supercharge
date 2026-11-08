//! Custom agents over ACP (`_supercharge/agents/*`).
//!
//! Studio's form and any other ACP client call these methods. The CLI writes
//! the same markdown the `/agents` form writes, then the client selects the
//! new id with `session/set_mode`.
//!
//! | Method | Params | Result |
//! |---|---|---|
//! | `agents/list` | `{sessionId}` | `{agents: [{id, name, description, scope}]}` |
//! | `agents/create` | `{sessionId, name, description?, instructions, scope?}` | `{id, name, description, scope, path}` |

use std::path::Path;

use agent_client_protocol as acp;
use serde_json::{Value, json};

use supercharge_agent::custom_agent::{CustomAgentDraft, CustomAgentScope, modes_for_cwd, write_custom_agent};

use crate::agent::mvp_agent::MvpAgent;
use crate::extensions::{ExtResult, to_raw_response};

pub(crate) const LIST: &str = "supercharge/agents/list";
pub(crate) const CREATE: &str = "supercharge/agents/create";

pub(crate) const METHODS: [&str; 2] = [LIST, CREATE];

fn invalid(msg: impl Into<String>) -> acp::Error {
    acp::Error::invalid_params().data(msg.into())
}

fn params_value(args: &acp::ExtRequest) -> Result<Value, acp::Error> {
    let raw = args.params.get();
    if raw.trim().is_empty() || raw.trim() == "null" {
        return Ok(json!({}));
    }
    serde_json::from_str(raw).map_err(|e| invalid(e.to_string()))
}

fn session_cwd(agent: &MvpAgent, params: &Value) -> Result<std::path::PathBuf, acp::Error> {
    let sid = params
        .get("sessionId")
        .and_then(Value::as_str)
        .map(acp::SessionId::new)
        .ok_or_else(|| invalid("sessionId required"))?;
    agent
        .get_session_cwd(&sid)
        .ok_or_else(|| invalid("session not found"))
}

fn scope_of(params: &Value) -> CustomAgentScope {
    match params.get("scope").and_then(Value::as_str) {
        Some("project") => CustomAgentScope::Project,
        _ => CustomAgentScope::User,
    }
}

fn scope_label(scope: supercharge_agent::config::AgentScope) -> &'static str {
    match scope {
        supercharge_agent::config::AgentScope::Project => "project",
        _ => "user",
    }
}

pub(crate) fn list_json(cwd: &Path) -> Value {
    let agents: Vec<Value> = modes_for_cwd(cwd)
        .into_iter()
        .map(|mode| {
            json!({
                "id": mode.id,
                "name": mode.name,
                "description": mode.description,
                "scope": scope_label(mode.scope),
            })
        })
        .collect();
    json!({ "agents": agents })
}

pub(crate) async fn handle(agent: &MvpAgent, method: &str, args: &acp::ExtRequest) -> ExtResult {
    let params = params_value(args)?;
    let cwd = session_cwd(agent, &params)?;
    match method {
        LIST => to_raw_response(&list_json(&cwd)),
        CREATE => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let description = params
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let instructions = params
                .get("instructions")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let draft = CustomAgentDraft {
                name,
                description,
                instructions,
                scope: scope_of(&params),
            };
            let path = write_custom_agent(&cwd, &draft).map_err(invalid)?;
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let created = modes_for_cwd(&cwd)
                .into_iter()
                .find(|mode| mode.id == id)
                .ok_or_else(|| acp::Error::internal_error().data("saved agent was not discovered"))?;
            to_raw_response(&json!({
                "id": created.id,
                "name": created.name,
                "description": created.description,
                "scope": scope_label(created.scope),
                "path": path.display().to_string(),
            }))
        }
        _ => Err(acp::Error::method_not_found()),
    }
}
