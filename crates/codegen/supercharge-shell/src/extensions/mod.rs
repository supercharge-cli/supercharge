pub mod agent_runtime;
pub mod auth;
pub(crate) mod auth_gate;
pub(crate) mod background_task;
pub(crate) mod setup;
pub mod billing;
pub mod btw;
pub mod bundle;
pub(crate) mod chat_conversation_history;
pub mod code_nav;
pub mod consent;
pub mod content;
pub mod debug;
pub mod feedback;
pub(crate) mod feedback_drafts;
pub(crate) mod feedback_trace;
pub mod fs;
pub mod git;
pub mod hooks;
pub mod hunk_tracker;
pub mod interject;
pub mod jj;
pub mod marketplace;
pub mod mcp;
pub mod memory;
pub mod notification;
pub mod plugins;
pub mod pr;
pub mod privacy;
pub mod prompt_history;
pub mod prompt_meta;
pub mod recap;
pub mod repair;
pub(crate) mod review;
pub mod rewind;
pub mod rollout;
pub mod routing;
pub mod search;
pub(crate) mod session_admin;
pub mod session_search;
pub(crate) mod session_state;
pub mod session_updates;
pub mod share;
pub mod skills;
pub mod subagent_message;
pub mod suggest;
pub mod task;
pub mod terminal;
pub mod usage;
pub mod worktree;
use crate::session::ExtMethodResult;
use agent_client_protocol as acp;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::sync::Arc;
pub type ExtResult = Result<acp::ExtResponse, acp::Error>;
pub(crate) fn parse_params<T: DeserializeOwned>(args: &acp::ExtRequest) -> Result<T, acp::Error> {
    parse_params_str(args.params.get())
}
/// Deserialize ACP params from their raw JSON string, mapping a parse failure to `invalid_params`.
/// Used by [`parse_params`] and the bridge `encode` hooks, which hold the params `RawValue` directly.
pub(crate) fn parse_params_str<T: DeserializeOwned>(raw: &str) -> Result<T, acp::Error> {
    serde_json::from_str(raw)
        .map_err(|e| acp::Error::invalid_params().data(format!("invalid params: {}", e)))
}
pub fn parse_session_id(args: &acp::ExtRequest) -> Option<acp::SessionId> {
    let v: serde_json::Value = serde_json::from_str(args.params.get()).ok()?;
    let sid = v.get("sessionId")?.as_str()?;
    Some(acp::SessionId::new(sid))
}
pub fn to_ext_response<T: Serialize>(result: anyhow::Result<T>) -> ExtResult {
    ExtMethodResult::from_result(result)
        .to_ext_response()
        .map_err(|e| acp::Error::internal_error().data(e.to_string()))
}
/// Wrap a serializable value as an `ExtResponse` without the `ExtMethodResult` envelope.
pub(crate) fn to_raw_response<T: Serialize>(v: &T) -> ExtResult {
    serde_json::value::to_raw_value(v)
        .map(|raw| acp::ExtResponse::new(Arc::from(raw)))
        .map_err(|e| acp::Error::internal_error().data(e.to_string()))
}
pub(crate) fn to_ext_response_partial<T: Serialize>(
    result: anyhow::Result<T>,
    warning: Option<String>,
) -> ExtResult {
    let ext_result = match (result, warning) {
        (Ok(data), Some(warn)) => ExtMethodResult::partial(data, warn),
        (Ok(data), None) => ExtMethodResult::success(data),
        (Err(e), _) => ExtMethodResult::failure(e),
    };
    ext_result
        .to_ext_response()
        .map_err(|e| acp::Error::internal_error().data(e.to_string()))
}
/// Empty response for operations that return no data.
#[derive(Debug, Serialize)]
pub struct Empty {}

/// Read a `_meta` entry by its legacy `x.ai/<rest>` key, preferring the
/// `_supercharge/<rest>` key when a client sends that instead.
// TODO(acp-rename): drop the legacy key once clients send only `_supercharge/*`.
pub(crate) fn meta_get<'a>(
    meta: &'a serde_json::Map<String, serde_json::Value>,
    legacy_key: &str,
) -> Option<&'a serde_json::Value> {
    legacy_key
        .strip_prefix("x.ai/")
        .and_then(|rest| meta.get(&format!("_supercharge/{rest}")))
        .or_else(|| meta.get(legacy_key))
}

/// Extension methods that are Supercharge-native (no legacy counterpart).
const NATIVE_SUPERCHARGE_METHODS: &[&str] = &[
    "supercharge/sessions/changed",
    "supercharge/models/list",
    "supercharge/session/context_window",
    "supercharge/session/thinking_probe",
    "supercharge/setup/status",
    "supercharge/setup/models",
    "supercharge/setup/save",
    "supercharge/setup/device_start",
    "supercharge/setup/device_poll",
    "supercharge/setup/device_cancel",
    "supercharge/setup/image_status",
    "supercharge/setup/image_save",
];

/// Map a Supercharge extension method name to the legacy name it aliases (both stay accepted).
///
/// Every `supercharge/<rest>` method maps to the legacy `x.ai/<rest>` handler, so clients
/// may send either `_supercharge/<rest>` or the legacy `_x.ai/<rest>` name.
// TODO(acp-rename): once Supercharge Studio sends only `_supercharge/*`, rename the handlers
// themselves and drop the legacy names.
pub(crate) fn legacy_alias(method: &str) -> Option<String> {
    if method == interject::STEER_METHOD || method == "supercharge/interject" {
        return Some(interject::INTERJECT_METHOD.to_string());
    }
    if NATIVE_SUPERCHARGE_METHODS.contains(&method) {
        return None;
    }
    if let Some(rest) = method.strip_prefix("supercharge/queue/") {
        return matches!(
            rest,
            "remove" | "reorder" | "clear" | "edit" | "interject" | "hold_edit" | "release_edit"
        )
        .then(|| format!("x.ai/queue/{rest}"));
    }
    let rest = method.strip_prefix("supercharge/")?;
    (!rest.is_empty()).then(|| format!("x.ai/{rest}"))
}

/// For an outgoing JSON-RPC *notification* (a `method` and no `id`) that still uses a
/// legacy `_x.ai/` or `x.ai/` method name, the same frame under the `_supercharge/` name.
/// Callers send this copy in addition to the original (dual-emit) so clients on either
/// name keep working. Requests and responses are never duplicated: a request answered
/// twice would break the client.
// TODO(acp-rename): drop the legacy copy once Supercharge Studio listens on `_supercharge/*` only.
pub(crate) fn supercharge_notification_alias(frame: &str) -> Option<String> {
    if !frame.contains("x.ai/") {
        return None;
    }
    let mut value: serde_json::Value = serde_json::from_str(frame).ok()?;
    let obj = value.as_object_mut()?;
    if obj.contains_key("id") {
        return None;
    }
    let method = obj.get("method")?.as_str()?;
    let rest = method
        .strip_prefix("_x.ai/")
        .or_else(|| method.strip_prefix("x.ai/"))?;
    let renamed = format!("_supercharge/{rest}");
    obj.insert("method".to_string(), serde_json::Value::String(renamed));
    serde_json::to_string(&value).ok()
}

#[cfg(test)]
mod alias_tests {
    use super::legacy_alias;

    #[test]
    fn supercharge_names_map_to_legacy_methods() {
        assert_eq!(
            legacy_alias("supercharge/session/steer").as_deref(),
            Some("x.ai/interject")
        );
        for op in ["remove", "reorder", "clear", "edit", "interject"] {
            assert_eq!(
                legacy_alias(&format!("supercharge/queue/{op}")),
                Some(format!("x.ai/queue/{op}"))
            );
        }
        assert_eq!(legacy_alias("supercharge/queue/bogus"), None);
        assert_eq!(legacy_alias("x.ai/interject"), None);
        assert_eq!(legacy_alias("supercharge/sessions/changed"), None);
        for m in [
            "capabilities",
            "fs_notify",
            "hooks",
            "mcp/sdk",
            "pluginDirs",
            "mcp/server_status",
            "session_notification",
            "interject",
        ] {
            assert_eq!(
                legacy_alias(&format!("supercharge/{m}")),
                Some(format!("x.ai/{m}"))
            );
        }
    }

    #[test]
    fn outgoing_notifications_get_supercharge_copy() {
        use super::supercharge_notification_alias as alias;
        let n = r#"{"jsonrpc":"2.0","method":"_x.ai/mcp/servers_updated","params":{"a":1}}"#;
        let v: serde_json::Value = serde_json::from_str(&alias(n).unwrap()).unwrap();
        assert_eq!(v["method"], "_supercharge/mcp/servers_updated");
        assert_eq!(v["params"]["a"], 1);
        let bare = r#"{"jsonrpc":"2.0","method":"x.ai/session_notification","params":{}}"#;
        let v: serde_json::Value = serde_json::from_str(&alias(bare).unwrap()).unwrap();
        assert_eq!(v["method"], "_supercharge/session_notification");
        // response_completed rides session_notification, so it also goes out under _supercharge/.
        let rc = r#"{"jsonrpc":"2.0","method":"_x.ai/session_notification","params":{"sessionId":"s","update":{"sessionUpdate":"response_completed"}}}"#;
        let v: serde_json::Value = serde_json::from_str(&alias(rc).unwrap()).unwrap();
        assert_eq!(v["method"], "_supercharge/session_notification");
        assert_eq!(v["params"]["update"]["sessionUpdate"], "response_completed");
        // Requests and responses are never duplicated.
        assert!(alias(r#"{"jsonrpc":"2.0","id":3,"method":"_x.ai/ask_user_question"}"#).is_none());
        assert!(alias(r#"{"jsonrpc":"2.0","id":3,"result":{"x.ai/k":1}}"#).is_none());
        assert!(alias(r#"{"jsonrpc":"2.0","method":"session/update"}"#).is_none());
    }
}
