//! Extension handlers for conversation compaction and the memory extensions
//! (`_supercharge/memory/{flush,dream,rewrite,forget}`; pre-rename names are still accepted).
//! `memory/rewrite` turns a raw memory note into structured markdown with a one-shot LLM call.
//! `memory/forget` deletes one note from the `/memory` modal through the store's tombstone path.

use agent_client_protocol as acp;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use super::{ExtResult, parse_params, to_raw_response};
use crate::agent::MvpAgent;
use crate::session::{CompactConversationRequest, CompactConversationResponse, SessionCommand};

#[tracing::instrument(skip_all, fields(method = %args.method))]
pub async fn handle(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    match args.method.as_ref() {
        m if m.starts_with("x.ai/compact_conversation") => handle_compact(agent, args).await,
        MEMORY_FLUSH_METHOD | "x.ai/memory/flush" => handle_flush(agent, args).await,
        MEMORY_DREAM_METHOD | "x.ai/memory/dream" => handle_dream(agent, args).await,
        MEMORY_REWRITE_METHOD | "x.ai/memory/rewrite" => handle_rewrite(agent, args).await,
        MEMORY_FORGET_METHOD | LEGACY_MEMORY_FORGET_METHOD => handle_forget(agent, args).await,
        _ => Err(acp::Error::method_not_found()),
    }
}

/// Primary ACP names for the memory extensions. The pre-rename names are
/// still accepted from older clients.
pub const MEMORY_FLUSH_METHOD: &str = "_supercharge/memory/flush";
pub const MEMORY_DREAM_METHOD: &str = "_supercharge/memory/dream";
pub const MEMORY_REWRITE_METHOD: &str = "_supercharge/memory/rewrite";
pub const MEMORY_FORGET_METHOD: &str = "_supercharge/memory/forget";
pub const LEGACY_MEMORY_FORGET_METHOD: &str = "x.ai/memory/forget";

/// Whether `method` is a memory extension under its primary name.
pub fn is_primary_memory_method(method: &str) -> bool {
    matches!(
        method,
        MEMORY_FLUSH_METHOD | MEMORY_DREAM_METHOD | MEMORY_REWRITE_METHOD | MEMORY_FORGET_METHOD
    )
}

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
