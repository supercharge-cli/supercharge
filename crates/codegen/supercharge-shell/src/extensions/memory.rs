//! Extension handlers for conversation compaction and the memory extensions
//! (`_supercharge/memory/{flush,dream,rewrite,forget,list,toggle}`; pre-rename names are still accepted).
//! `memory/rewrite` turns a raw memory note into structured markdown with a one-shot LLM call.
//! `memory/forget` deletes one note from the `/memory` modal through the store's tombstone path.
//! `memory/list` and `memory/toggle` let the `/memory` modal read and flip state without a turn.

use agent_client_protocol as acp;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use super::{ExtResult, parse_params, to_raw_response};
use super::notification::{MemoryDisabledReason, MemoryFileInfo};
use crate::agent::MvpAgent;
use crate::session::{CompactConversationRequest, CompactConversationResponse, SessionCommand};

#[tracing::instrument(skip_all, fields(method = %args.method))]
pub async fn handle(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    match args.method.as_ref() {
        m if m.starts_with("x.ai/compact_conversation") => handle_compact(agent, args).await,
        m => match memory_method(m) {
            Some(MemoryMethod::Flush) => handle_flush(agent, args).await,
            Some(MemoryMethod::Dream) => handle_dream(agent, args).await,
            Some(MemoryMethod::Rewrite) => handle_rewrite(agent, args).await,
            Some(MemoryMethod::Forget) => handle_forget(agent, args).await,
            Some(MemoryMethod::List) => handle_list(agent, args).await,
            Some(MemoryMethod::Toggle) => handle_toggle(agent, args).await,
            None => Err(acp::Error::method_not_found()),
        },
    }
}

/// A memory extension, independent of which ACP name the client used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryMethod {
    Flush,
    Dream,
    Rewrite,
    Forget,
    List,
    Toggle,
}

/// Resolve a memory extension by its primary or pre-rename ACP name.
pub fn memory_method(method: &str) -> Option<MemoryMethod> {
    match method {
        MEMORY_FLUSH_METHOD | "x.ai/memory/flush" => Some(MemoryMethod::Flush),
        MEMORY_DREAM_METHOD | "x.ai/memory/dream" => Some(MemoryMethod::Dream),
        MEMORY_REWRITE_METHOD | "x.ai/memory/rewrite" => Some(MemoryMethod::Rewrite),
        MEMORY_FORGET_METHOD | LEGACY_MEMORY_FORGET_METHOD => Some(MemoryMethod::Forget),
        MEMORY_LIST_METHOD | "x.ai/memory/list" => Some(MemoryMethod::List),
        MEMORY_TOGGLE_METHOD | "x.ai/memory/toggle" => Some(MemoryMethod::Toggle),
        _ => None,
    }
}

/// Primary ACP names for the memory extensions. The pre-rename names are
/// still accepted from older clients.
pub const MEMORY_FLUSH_METHOD: &str = "_supercharge/memory/flush";
pub const MEMORY_DREAM_METHOD: &str = "_supercharge/memory/dream";
pub const MEMORY_REWRITE_METHOD: &str = "_supercharge/memory/rewrite";
pub const MEMORY_FORGET_METHOD: &str = "_supercharge/memory/forget";
pub const MEMORY_LIST_METHOD: &str = "_supercharge/memory/list";
pub const MEMORY_TOGGLE_METHOD: &str = "_supercharge/memory/toggle";
pub const LEGACY_MEMORY_FORGET_METHOD: &str = "x.ai/memory/forget";

/// Largest note `_supercharge/memory/forget` will hash and delete, for both v2 and legacy stores.
pub const MEMORY_FORGET_MAX_FILE_BYTES: u64 = supercharge_memory::MAX_FORGET_FILE_BYTES;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryForgetRequest {
    pub session_id: String,
    /// Absolute path as listed by `MemoryFiles`.
    pub path: String,
    /// BLAKE3 hex of the bytes the user previewed; the store refuses to delete anything else.
    pub expected_content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum MemoryForgetResponse {
    Forgotten {
        was_already_forgotten: bool,
    },
    Rejected {
        reason: MemoryForgetRejection,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryForgetRejection {
    MemoryDisabled,
    /// Manifests, files outside the store, or anything the store's access policy protects.
    NotDeletable,
    /// The file no longer matches the previewed bytes.
    Changed,
    /// Dream holds the consolidation lease; retry once it finishes.
    DreamRunning,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryListRequest {
    pub session_id: String,
}

/// What the `/memory` modal renders: the same payload as the `MemoryFiles` notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryListing {
    pub files: Vec<MemoryFileInfo>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<MemoryDisabledReason>,
    pub capture_enabled: bool,
    pub dream_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryToggleRequest {
    pub session_id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryToggleResponse {
    /// Outcome for the user (also covers "already on" and refusals).
    pub message: String,
    /// State after the toggle; refusals leave it unchanged.
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<MemoryDisabledReason>,
    /// Files after the toggle, so an open modal can refresh without a second round trip.
    /// `None` when the store could not be listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing: Option<MemoryListing>,
}

fn resident_session(
    agent: &MvpAgent,
    session_id: String,
) -> Result<crate::session::SessionHandle, acp::Error> {
    agent
        .resident_handle(&acp::SessionId::new(session_id.clone()))
        .ok_or_else(|| acp::Error::invalid_params().data(format!("session not found: {session_id}")))
}

async fn handle_list(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    let req: MemoryListRequest = parse_params(args)?;
    let session = resident_session(agent, req.session_id)?;
    let (tx, rx) = oneshot::channel();
    let _ = session
        .cmd_tx
        .send(SessionCommand::MemoryList { respond_to: tx });
    let listing = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?
        .map_err(|e| acp::Error::internal_error().data(e))?;
    to_raw_response(&listing)
}

async fn handle_toggle(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    let req: MemoryToggleRequest = parse_params(args)?;
    let session = resident_session(agent, req.session_id)?;
    let (tx, rx) = oneshot::channel();
    let _ = session.cmd_tx.send(SessionCommand::MemoryToggle {
        enabled: req.enabled,
        respond_to: tx,
    });
    let response = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?;
    to_raw_response(&response)
}

async fn handle_forget(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    let req: MemoryForgetRequest = parse_params(args)?;
    let not_found_err = format!("session not found: {}", req.session_id);
    let sid: acp::SessionId = req.session_id.into();
    let Some(session) = agent.resident_handle(&sid) else {
        return Err(acp::Error::invalid_params().data(not_found_err));
    };
    let (tx, rx) = oneshot::channel();
    let _ = session.cmd_tx.send(SessionCommand::MemoryForget {
        path: req.path,
        expected_content_hash: req.expected_content_hash,
        respond_to: tx,
    });
    let response = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?;
    to_raw_response(&response)
}

async fn handle_compact(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    let req: CompactConversationRequest = parse_params(args)?;
    // send over the compact query here properly
    let sid: acp::SessionId = req.session_id.into();
    let session_handle = agent.resident_handle(&sid);
    let (tx, rx) = oneshot::channel();
    if let Some(session) = session_handle {
        let _ = session.cmd_tx.send(SessionCommand::CompactSession {
            user_context: req.user_context,
            respond_to: tx,
        });
    }
    // Pass the session error through; rewrapping buries the detail in a Debug dump.
    rx.await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))??;
    to_raw_response(&CompactConversationResponse {})
}

async fn handle_flush(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    #[derive(Deserialize)]
    struct MemoryFlushRequest {
        session_id: String,
    }

    let req: MemoryFlushRequest = parse_params(args)?;
    let not_found_err = format!("session not found: {}", req.session_id);
    let sid: acp::SessionId = req.session_id.into();
    let Some(session) = agent.resident_handle(&sid) else {
        return Err(acp::Error::invalid_params().data(not_found_err));
    };
    let (tx, rx) = oneshot::channel();
    let _ = session
        .cmd_tx
        .send(SessionCommand::FlushMemory { respond_to: tx });
    let flushed = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?
        .map_err(|e| acp::Error::internal_error().data(format!("{:?}", e)))?;
    to_raw_response(&MemoryFlushResponse { flushed })
}

#[derive(Serialize)]
struct MemoryFlushResponse {
    flushed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryDreamOutcome {
    Completed,
    NoWork,
    Busy,
    Failed,
    Disabled,
}

async fn handle_dream(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    #[derive(Deserialize)]
    struct Request {
        session_id: String,
    }
    let req: Request = parse_params(args)?;
    let sid: acp::SessionId = req.session_id.into();
    let session = agent
        .resident_handle(&sid)
        .ok_or_else(|| acp::Error::invalid_params().data("unknown session id"))?;
    let (tx, rx) = oneshot::channel();
    session
        .cmd_tx
        .send(SessionCommand::DreamMemory { respond_to: tx })
        .map_err(|_| acp::Error::internal_error().data("session closed"))?;
    let outcome = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?;
    to_raw_response(&serde_json::json!({ "outcome": outcome }))
}

async fn handle_rewrite(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RewriteRequest {
        session_id: String,
        raw_text: String,
        context_summary: String,
    }

    let req: RewriteRequest = parse_params(args)?;
    let not_found_err = format!("session not found: {}", req.session_id);
    let sid: acp::SessionId = req.session_id.into();
    let Some(session) = agent.resident_handle(&sid) else {
        return Err(acp::Error::invalid_params().data(not_found_err));
    };
    let (tx, rx) = oneshot::channel();
    let _ = session.cmd_tx.send(SessionCommand::RewriteMemoryNote {
        raw_text: req.raw_text,
        context_summary: req.context_summary,
        respond_to: tx,
    });
    let rewritten = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?
        .map_err(|e| acp::Error::internal_error().data(e))?;
    to_raw_response(&serde_json::json!({ "rewritten": rewritten }))
}

#[cfg(test)]
mod dream_response_tests {
    use super::*;

    #[test]
    fn dream_outcomes_serialize_as_typed_dispositions() {
        for (outcome, label) in [
            (MemoryDreamOutcome::Completed, "completed"),
            (MemoryDreamOutcome::NoWork, "no_work"),
            (MemoryDreamOutcome::Busy, "busy"),
            (MemoryDreamOutcome::Failed, "failed"),
            (MemoryDreamOutcome::Disabled, "disabled"),
        ] {
            let value = serde_json::to_value(outcome).unwrap();
            assert_eq!(value, label);
            assert_eq!(
                serde_json::from_value::<MemoryDreamOutcome>(value).unwrap(),
                outcome
            );
        }
    }
}

#[cfg(test)]
mod method_name_tests {
    use super::*;

    #[test]
    fn primary_and_pre_rename_memory_methods_are_accepted() {
        for (primary, legacy, kind) in [
            (MEMORY_FLUSH_METHOD, "x.ai/memory/flush", MemoryMethod::Flush),
            (MEMORY_DREAM_METHOD, "x.ai/memory/dream", MemoryMethod::Dream),
            (MEMORY_REWRITE_METHOD, "x.ai/memory/rewrite", MemoryMethod::Rewrite),
            (MEMORY_FORGET_METHOD, "x.ai/memory/forget", MemoryMethod::Forget),
            (MEMORY_LIST_METHOD, "x.ai/memory/list", MemoryMethod::List),
            (MEMORY_TOGGLE_METHOD, "x.ai/memory/toggle", MemoryMethod::Toggle),
        ] {
            assert!(primary.starts_with("_supercharge/memory/"));
            assert_eq!(memory_method(primary), Some(kind), "{primary}");
            assert_eq!(memory_method(legacy), Some(kind), "{legacy}");
        }
        assert_eq!(memory_method("_supercharge/memory/unknown"), None);
        assert_eq!(memory_method("x.ai/memory/unknown"), None);
    }

    #[test]
    fn list_and_toggle_shapes() {
        let req: MemoryToggleRequest =
            serde_json::from_value(serde_json::json!({"sessionId": "s1", "enabled": false}))
                .unwrap();
        assert_eq!(req.session_id, "s1");
        assert!(!req.enabled);
        let listing = MemoryListing {
            files: vec![],
            enabled: false,
            disabled_reason: Some(MemoryDisabledReason::SessionToggle),
            capture_enabled: true,
            dream_enabled: true,
        };
        let response = MemoryToggleResponse {
            message: "Memory disabled for this session.".into(),
            enabled: false,
            disabled_reason: Some(MemoryDisabledReason::SessionToggle),
            listing: Some(listing.clone()),
        };
        let v = serde_json::to_value(&response).unwrap();
        assert_eq!(v["disabled_reason"], "session_toggle");
        assert_eq!(v["listing"]["capture_enabled"], true);
        assert_eq!(
            serde_json::from_value::<MemoryToggleResponse>(v).unwrap(),
            response
        );
        let no_listing = MemoryToggleResponse {
            listing: None,
            disabled_reason: None,
            enabled: true,
            ..response
        };
        let v = serde_json::to_value(&no_listing).unwrap();
        assert!(v.get("listing").is_none() && v.get("disabled_reason").is_none());
    }

    #[test]
    fn memory_mode_meta_has_primary_and_legacy_keys() {
        assert_eq!(crate::session::MEMORY_MODE_META_KEY, "_supercharge/memoryMode");
        assert_eq!(crate::session::LEGACY_MEMORY_MODE_META_KEY, "x.ai/memoryMode");
    }
}
