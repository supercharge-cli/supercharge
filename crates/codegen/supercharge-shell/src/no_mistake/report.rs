//! Pipeline report, step names, and process exit codes.

use serde::Serialize;

use crate::no_mistake::findings::Finding;

/// A step in the Supercharge validation gate. Order is fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepName {
    Intent,
    Isolate,
    Rebase,
    Review,
    Test,
    Document,
    Lint,
    Push,
    Pr,
    Ci,
}

impl StepName {
    pub const ALL: [StepName; 10] = [
        Self::Intent,
        Self::Isolate,
        Self::Rebase,
        Self::Review,
        Self::Test,
        Self::Document,
        Self::Lint,
        Self::Push,
        Self::Pr,
        Self::Ci,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intent => "intent",
            Self::Isolate => "isolate",
            Self::Rebase => "rebase",
            Self::Review => "review",
            Self::Test => "test",
            Self::Document => "document",
            Self::Lint => "lint",
            Self::Push => "push",
            Self::Pr => "pr",
            Self::Ci => "ci",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "intent" => Some(Self::Intent),
            "isolate" => Some(Self::Isolate),
            "rebase" => Some(Self::Rebase),
            "review" => Some(Self::Review),
            "test" => Some(Self::Test),
            "document" | "docs" => Some(Self::Document),
            "lint" => Some(Self::Lint),
            "push" => Some(Self::Push),
            "pr" => Some(Self::Pr),
            "ci" => Some(Self::Ci),
            _ => None,
        }
    }

    pub fn all_csv() -> &'static str {
        "intent, isolate, rebase, review, test, document, lint, push, pr, ci"
    }

    pub fn is_publish(self) -> bool {
        matches!(self, Self::Push | Self::Pr | Self::Ci)
    }

    pub fn is_local_gate(self) -> bool {
        !self.is_publish()
    }
}

/// Result of one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepStatus {
    Pending,
    Running,
    Passed,
    Skipped,
    Failed,
    Cancelled,
    NeedsReview,
}

impl StepStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Passed => "passed",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::NeedsReview => "needs-review",
        }
    }
}

/// Final gate outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Passed,
    PassedWithSkips,
    Failed,
    Cancelled,
    NeedsReview,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::PassedWithSkips => "passed-with-skips",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::NeedsReview => "needs-review",
        }
    }

    pub fn is_success(self) -> bool {
        matches!(self, Self::Passed | Self::PassedWithSkips)
    }
}

/// Process exit code for a gate outcome. Non-zero on anything other than a pass.
pub fn exit_code(outcome: Outcome) -> i32 {
    match outcome {
        Outcome::Passed | Outcome::PassedWithSkips => 0,
        Outcome::NeedsReview => 2,
        Outcome::Cancelled => 130,
        Outcome::Failed => 1,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StepReport {
    pub name: StepName,
    pub status: StepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
}

impl StepReport {
    pub fn new(name: StepName, status: StepStatus) -> Self {
        Self {
            name,
            status,
            detail: None,
            findings: Vec::new(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Complete report for one gate run.
#[derive(Debug, Clone, Serialize)]
pub struct GateReport {
    pub outcome: Outcome,
    pub intent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    pub steps: Vec<StepReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl GateReport {
    pub fn render_plain(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Supercharge no-mistake: {}\n",
            self.outcome.as_str()
        ));
        if !self.intent.is_empty() {
            out.push_str(&format!("Intent: {}\n", self.intent));
        }
        if let Some(branch) = &self.branch {
            out.push_str(&format!("Branch: {branch}\n"));
        }
        if let Some(base) = &self.base_branch {
            out.push_str(&format!("Base: {base}\n"));
        }
        if let Some(wt) = &self.worktree {
            out.push_str(&format!("Worktree: {wt}\n"));
        }
        out.push('\n');
        for step in &self.steps {
            let mark = match step.status {
                StepStatus::Passed => "ok",
                StepStatus::Skipped => "skipped",
                StepStatus::Failed => "failed",
                StepStatus::Cancelled => "cancelled",
                StepStatus::NeedsReview => "needs review",
                StepStatus::Pending => "pending",
                StepStatus::Running => "running",
            };
            out.push_str(&format!("  [{mark}] {}\n", step.name.as_str()));
            if let Some(detail) = &step.detail {
                out.push_str(&format!("         {detail}\n"));
            }
            for finding in &step.findings {
                let loc = match (&finding.file, finding.line) {
                    (Some(file), Some(line)) => format!(" {file}:{line}"),
                    (Some(file), None) => format!(" {file}"),
                    _ => String::new(),
                };
                out.push_str(&format!(
                    "         - {} {}{}: {}\n",
                    finding.severity.as_str(),
                    finding.action.as_str(),
                    loc,
                    finding.description
                ));
            }
        }
        if let Some(message) = &self.message {
            out.push('\n');
            out.push_str(message);
            if !message.ends_with('\n') {
                out.push('\n');
            }
        }
        out
    }

    pub fn render_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn render(&self, kind: crate::no_mistake::args::OutputKind) -> String {
        match kind {
            crate::no_mistake::args::OutputKind::Plain => self.render_plain(),
            crate::no_mistake::args::OutputKind::Json => {
                self.render_json().unwrap_or_else(|_| self.render_plain())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_aliases_and_unknown() {
        assert_eq!(StepName::parse("docs"), Some(StepName::Document));
        assert_eq!(StepName::parse("PR"), Some(StepName::Pr));
        assert_eq!(StepName::parse("nope"), None);
    }

    #[test]
    fn success_exit_codes() {
        assert_eq!(exit_code(Outcome::Passed), 0);
        assert_eq!(exit_code(Outcome::PassedWithSkips), 0);
        assert_eq!(exit_code(Outcome::NeedsReview), 2);
        assert_eq!(exit_code(Outcome::Failed), 1);
        assert_eq!(exit_code(Outcome::Cancelled), 130);
    }
}
