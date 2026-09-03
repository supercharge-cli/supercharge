//! ACP initialize / session helpers. Supercharge identity only.

use std::path::{Path, PathBuf};

use agent_client_protocol as acp;

/// Wire `clientType` / `clientIdentifier` for this app.
pub const CLIENT_IDENTIFIER: &str = "supercharge-desktop";
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn initialize_request() -> acp::InitializeRequest {
    let mut meta = acp::Meta::new();
    meta.insert(
        "clientType".into(),
        serde_json::Value::String(CLIENT_IDENTIFIER.to_string()),
    );
    meta.insert(
        "clientIdentifier".into(),
        serde_json::Value::String(CLIENT_IDENTIFIER.to_string()),
    );
    meta.insert(
        "clientVersion".into(),
        serde_json::Value::String(CLIENT_VERSION.to_string()),
    );

    acp::InitializeRequest::new(acp::ProtocolVersion::V1)
        .client_capabilities(
            acp::ClientCapabilities::new()
                .fs(
                    acp::FileSystemCapabilities::new()
                        .read_text_file(true)
                        .write_text_file(true),
                )
                .terminal(false),
        )
        .meta(meta)
}

pub fn new_session_request(cwd: PathBuf) -> acp::NewSessionRequest {
    acp::NewSessionRequest::new(cwd).mcp_servers(Vec::new())
}

pub fn prompt_request(session_id: acp::SessionId, text: String) -> acp::PromptRequest {
    acp::PromptRequest::new(
        session_id,
        vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
    )
}

pub fn content_text(block: &acp::ContentBlock) -> Option<&str> {
    match block {
        acp::ContentBlock::Text(t) => Some(t.text.as_str()),
        _ => None,
    }
}

pub fn format_session_update(update: &acp::SessionUpdate) -> Option<TranscriptLine> {
    match update {
        acp::SessionUpdate::AgentMessageChunk(chunk) => {
            content_text(&chunk.content).map(|t| TranscriptLine {
                kind: LineKind::Agent,
                text: t.to_string(),
                ..TranscriptLine::default()
            })
        }
        acp::SessionUpdate::AgentThoughtChunk(chunk) => {
            content_text(&chunk.content).map(|t| TranscriptLine {
                kind: LineKind::Thought,
                text: t.to_string(),
                ..TranscriptLine::default()
            })
        }
        acp::SessionUpdate::UserMessageChunk(chunk) => {
            content_text(&chunk.content).map(|t| TranscriptLine {
                kind: LineKind::User,
                text: t.to_string(),
                ..TranscriptLine::default()
            })
        }
        acp::SessionUpdate::ToolCall(call) => Some(TranscriptLine {
            kind: LineKind::Tool,
            text: format!("{:?} {}", call.kind, call.title),
            ..TranscriptLine::default()
        }),
        acp::SessionUpdate::ToolCallUpdate(update) => {
            let title = update.fields.title.clone().unwrap_or_default();
            if title.is_empty() {
                None
            } else {
                Some(TranscriptLine {
                    kind: LineKind::Tool,
                    text: title,
                    ..TranscriptLine::default()
                })
            }
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    User,
    Agent,
    Thought,
    Tool,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptLine {
    pub kind: LineKind,
    pub text: String,
    /// Per-line UI state: expanded/collapsed for thought + tool entries.
    pub tool_open: Option<bool>,
    /// Render as an error (system lines only).
    pub error: bool,
}

impl Default for TranscriptLine {
    fn default() -> Self {
        Self {
            kind: LineKind::System,
            text: String::new(),
            tool_open: None,
            error: false,
        }
    }
}

/// Locate the Supercharge agent binary that speaks ACP stdio.
pub fn resolve_agent_bin(explicit: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(path) = explicit {
        anyhow::ensure!(path.is_file(), "agent binary not found: {}", path.display());
        return Ok(path.to_path_buf());
    }
    if let Ok(from_env) = std::env::var("SUPERCHARGE_BIN") {
        let path = PathBuf::from(from_env);
        anyhow::ensure!(
            path.is_file(),
            "SUPERCHARGE_BIN is not a file: {}",
            path.display()
        );
        return Ok(path);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for name in ["supercharge-pager", "supercharge"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    for name in ["supercharge", "supercharge-pager"] {
        if let Ok(path) = which(name) {
            return Ok(path);
        }
    }
    anyhow::bail!(
        "could not find Supercharge agent binary. Pass --bin, set SUPERCHARGE_BIN, or put `supercharge` / `supercharge-pager` on PATH."
    )
}

fn which(name: &str) -> anyhow::Result<PathBuf> {
    let path = std::env::var_os("PATH").ok_or_else(|| anyhow::anyhow!("PATH is unset"))?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    anyhow::bail!("{name} not on PATH")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_identifies_as_supercharge_desktop() {
        let req = initialize_request();
        let meta = req.meta.as_ref().expect("meta");
        assert_eq!(
            meta.get("clientType").and_then(|v| v.as_str()),
            Some("supercharge-desktop")
        );
        assert_eq!(
            meta.get("clientIdentifier").and_then(|v| v.as_str()),
            Some("supercharge-desktop")
        );
        let json = serde_json::to_string(&req).expect("serialize");
        assert!(!json.contains("grok-desktop"));
        assert!(json.contains("supercharge-desktop"));
    }

    #[test]
    fn prompt_wraps_plain_text() {
        let req = prompt_request(acp::SessionId::new("s1"), "hello".into());
        assert_eq!(req.prompt.len(), 1);
        assert_eq!(content_text(&req.prompt[0]), Some("hello"));
    }

    #[test]
    fn formats_agent_text_chunk() {
        let update = acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
            acp::ContentBlock::Text(acp::TextContent::new("hi")),
        ));
        let line = format_session_update(&update).expect("line");
        assert_eq!(line.kind, LineKind::Agent);
        assert_eq!(line.text, "hi");
    }
}
