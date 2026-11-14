//! Advisors that produce review findings and optional fixes.

use std::path::Path;

use crate::no_mistake::findings::{Finding, FindingAction, Severity};
use crate::no_mistake::pipeline::GateError;

/// Context for a review pass.
pub struct ReviewContext<'a> {
    pub intent: &'a str,
    pub branch: &'a str,
    pub base_branch: &'a str,
    pub worktree: &'a Path,
    pub diff: &'a str,
}

/// Context for a fix pass.
pub struct FixContext<'a> {
    pub intent: &'a str,
    pub worktree: &'a Path,
    pub findings: &'a [Finding],
}

/// Optional model or script used by review, test, and lint steps.
pub trait Advisor {
    fn review(&self, ctx: &ReviewContext<'_>) -> Result<Vec<Finding>, GateError>;

    /// Apply requested auto-fix findings. Return true if the tree may have changed.
    fn apply_fixes(&self, ctx: &FixContext<'_>) -> Result<bool, GateError> {
        let _ = ctx;
        Ok(false)
    }
}

/// Built-in reviewer: conservative static checks on the branch diff. No network.
pub struct StaticAdvisor;

impl Advisor for StaticAdvisor {
    fn review(&self, ctx: &ReviewContext<'_>) -> Result<Vec<Finding>, GateError> {
        Ok(static_review(ctx.diff))
    }
}

/// Advisor used in tests: returns a prepared finding list and optionally "applies" fixes.
pub struct ScriptedAdvisor {
    pub findings: Vec<Finding>,
    pub apply: bool,
}

impl Advisor for ScriptedAdvisor {
    fn review(&self, _ctx: &ReviewContext<'_>) -> Result<Vec<Finding>, GateError> {
        Ok(self.findings.clone())
    }

    fn apply_fixes(&self, ctx: &FixContext<'_>) -> Result<bool, GateError> {
        if !self.apply {
            return Ok(false);
        }
        // Leave a marker so tests can see that a fix pass ran.
        let marker = ctx.worktree.join(".supercharge-no-mistake-fix");
        let ids: Vec<&str> = ctx.findings.iter().map(|f| f.id.as_str()).collect();
        std::fs::write(&marker, ids.join(","))
            .map_err(|e| GateError::io("write the scripted fix marker", e))?;
        Ok(true)
    }
}

pub fn static_review(diff: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut n = 0u32;
    if diff.trim().is_empty() {
        return findings;
    }
    for (idx, line) in diff.lines().enumerate() {
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff ") {
            continue;
        }
        let added = line.starts_with('+');
        if !added {
            continue;
        }
        let content = line.get(1..).unwrap_or("");
        if content.contains("<<<<<<<")
            || content.contains(">>>>>>>")
            || content.contains("======= ")
            || content == "======="
        {
            n = n.saturating_add(1);
            findings.push(
                Finding::new(
                    format!("conflict-{n}"),
                    Severity::Error,
                    FindingAction::AskUser,
                    "The diff still contains a merge conflict marker.",
                )
                .at_file(format!("diff:{idx}")),
            );
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_conflict_markers_only_on_added_lines() {
        let diff = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@\n-old\n+<<<<<<< HEAD\n+yours\n";
        let findings = static_review(diff);
        assert_eq!(findings.len(), 1);
        let finding = findings.iter().next().expect("one finding");
        assert_eq!(finding.action, FindingAction::AskUser);
        assert_eq!(finding.severity, Severity::Error);
    }

    #[test]
    fn clean_diff_has_no_findings() {
        let diff = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@\n-old\n+new\n";
        assert!(static_review(diff).is_empty());
    }
}
