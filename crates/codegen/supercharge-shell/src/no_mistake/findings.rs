//! Findings returned by a pipeline step.

use serde::Serialize;

/// How serious a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }

    pub fn is_blocking(self) -> bool {
        matches!(self, Self::Error)
    }
}

/// What the gate may do with a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingAction {
    AutoFix,
    AskUser,
    NoOp,
}

impl FindingAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AutoFix => "auto-fix",
            Self::AskUser => "ask-user",
            Self::NoOp => "no-op",
        }
    }
}

/// One issue raised by a pipeline step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub id: String,
    pub severity: Severity,
    pub action: FindingAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub description: String,
}

impl Finding {
    pub fn new(
        id: impl Into<String>,
        severity: Severity,
        action: FindingAction,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            severity,
            action,
            file: None,
            line: None,
            description: description.into(),
        }
    }

    pub fn at_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }
}
