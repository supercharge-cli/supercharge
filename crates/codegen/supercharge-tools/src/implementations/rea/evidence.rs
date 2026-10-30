//! Evidence-shaped replies: a normalized result plus explicit limitations.
//! Adapted in spirit from morluto/rea (MIT): every conclusion carries what was
//! observed and what remains unknown.

use serde::Serialize;

/// One investigation reply. Serialize this as the tool's text (pretty JSON).
#[derive(Debug, Clone, Serialize)]
pub struct EvidenceReply<T: Serialize> {
    pub operation: &'static str,
    pub path: String,
    pub normalized_result: T,
    pub limitations: Vec<String>,
    /// Always true for target-derived content; mirrors REA's provenance marker.
    pub untrusted_content: bool,
}

impl<T: Serialize> EvidenceReply<T> {
    pub fn ok(operation: &'static str, path: impl Into<String>, result: T) -> Self {
        Self {
            operation,
            path: path.into(),
            normalized_result: result,
            limitations: Vec::new(),
            untrusted_content: true,
        }
    }

    pub fn with_limitation(mut self, note: impl Into<String>) -> Self {
        self.limitations.push(note.into());
        self
    }

    pub fn to_json_text(&self) -> String {
        match serde_json::to_string_pretty(self) {
            Ok(s) => truncate_output(s),
            Err(e) => format!(
                "{{\"operation\":\"{}\",\"error\":\"serialize failed: {e}\",\"untrusted_content\":true}}",
                self.operation
            ),
        }
    }
}

/// Error-shaped reply that never panics the tool handler.
#[derive(Debug, Clone, Serialize)]
pub struct EvidenceError {
    pub operation: &'static str,
    pub path: Option<String>,
    pub error: String,
    pub limitations: Vec<String>,
    pub untrusted_content: bool,
}

impl EvidenceError {
    pub fn new(operation: &'static str, path: Option<String>, error: impl Into<String>) -> Self {
        Self {
            operation,
            path,
            error: error.into(),
            limitations: Vec::new(),
            untrusted_content: true,
        }
    }

    pub fn to_json_text(&self) -> String {
        match serde_json::to_string_pretty(self) {
            Ok(s) => truncate_output(s),
            Err(e) => format!("{{\"error\":\"serialize failed: {e}\"}}"),
        }
    }
}

fn truncate_output(s: String) -> String {
    use super::bounds::MAX_OUTPUT_CHARS;
    if s.len() <= MAX_OUTPUT_CHARS {
        return s;
    }
    let mut cut = MAX_OUTPUT_CHARS.saturating_sub(80);
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!(
        "{}\n… [truncated: output exceeded {} chars; narrow the request]",
        &s[..cut],
        MAX_OUTPUT_CHARS
    )
}
