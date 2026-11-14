//! Supercharge-authored prompts for agent-backed gate steps.

use crate::no_mistake::findings::Finding;

/// Review prompt for an optional model-backed advisor. Written by Supercharge.
pub fn review_prompt(intent: &str, branch: &str, base: &str, diff: &str) -> String {
    let intent = if intent.is_empty() {
        "(not provided)"
    } else {
        intent
    };
    let diff = truncate(diff, 80_000);
    format!(
        "You are the Supercharge validation-gate reviewer.\n\
         Review the branch diff below against the stated intent.\n\
         Do not push, open a pull request, or start another /no-mistake run.\n\
         Return only a JSON array of findings. Each finding must be an object with keys:\n\
         id, severity (error|warning|info), action (auto-fix|ask-user|no-op), description,\n\
         and optional file and line.\n\
         If there are no issues, return [].\n\n\
         Intent: {intent}\n\
         Branch: {branch}\n\
         Base: {base}\n\n\
         Diff:\n{diff}\n"
    )
}

/// Fix prompt used when the gate asks an advisor to apply selected findings.
pub fn fix_prompt(intent: &str, findings: &[Finding]) -> String {
    let mut body = String::from(
        "You are applying Supercharge validation-gate fixes in the current worktree.\n\
         Change only what is needed to address the findings listed below.\n\
         Do not start another /no-mistake run, push, or open a pull request.\n\
         After editing, stop.\n\n",
    );
    if !intent.is_empty() {
        body.push_str("Intent: ");
        body.push_str(intent);
        body.push_str("\n\n");
    }
    body.push_str("Findings:\n");
    for finding in findings {
        body.push_str(&format!(
            "- [{}] {} ({}): {}\n",
            finding.id,
            finding.severity.as_str(),
            finding.action.as_str(),
            finding.description
        ));
    }
    body
}

fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        text
    } else {
        text.get(..max).unwrap_or(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_prompt_mentions_supercharge_only() {
        let p = review_prompt("add flag", "feat/x", "main", "diff --git a/x");
        assert!(p.contains("Supercharge"));
        assert!(p.contains("add flag"));
        assert!(!p.to_ascii_lowercase().contains("no-mistakes"));
    }
}
