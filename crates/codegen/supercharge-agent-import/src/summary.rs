use std::fmt::Write as _;
use std::path::Path;

use super::find_project_root;
use super::items::{ImportPlan, ImportRuleAction, ImportableItem, PathKind, format_rule_string};

impl ImportPlan {
    /// Human-readable summary of the plan. Environment values are redacted.
    #[must_use]
    pub fn summary(&self, cwd: &Path) -> String {
        if self.is_empty() {
            return "No settings from other coding agents found to import.".to_string();
        }
        let mut out = String::from("Found settings from other coding agents to import:\n");
        if !self.global_items.is_empty() {
            out.push_str("\nGlobal (~/.supercharge/config.toml):\n");
            out.push_str(&format_item_summary(&self.global_items));
        }
        if !self.project_items.is_empty() {
            let _ = writeln!(
                out,
                "\nProject ({}/.supercharge/config.toml):",
                find_project_root(cwd).display()
            );
            out.push_str(&format_item_summary(&self.project_items));
        }
        out
    }
}

/// Items grouped by kind, one detail line per item.
fn format_item_summary(items: &[ImportableItem]) -> String {
    let mut out = String::new();

    let perms: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            ImportableItem::Permission(rule) => Some(rule),
            _ => None,
        })
        .collect();
    if !perms.is_empty() {
        let parts: Vec<String> = [
            ImportRuleAction::Allow,
            ImportRuleAction::Deny,
            ImportRuleAction::Ask,
        ]
        .into_iter()
        .filter_map(|action| {
            let n = perms.iter().filter(|rule| rule.action == action).count();
            (n > 0).then(|| format!("{n} {}", action.as_str()))
        })
        .collect();
        let _ = writeln!(
            out,
            "  - {} permission rule(s) ({})",
            perms.len(),
            parts.join(", ")
        );
        for rule in &perms {
            let _ = writeln!(
                out,
                "      {} {}",
                rule.action.as_str(),
                format_rule_string(rule)
            );
        }
    }

    let envs: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            ImportableItem::EnvVar { key, value } => Some((key, value)),
            _ => None,
        })
        .collect();
    if !envs.is_empty() {
        let _ = writeln!(out, "  - {} environment variable(s)", envs.len());
        for (key, value) in &envs {
            // Values can hold tokens; show only the length so the user can recognise it.
            let _ = writeln!(
                out,
                "      {key} = <redacted, {} chars>",
                value.chars().count()
            );
        }
    }

    let mcps: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            ImportableItem::McpServer { name, .. } => Some(name),
            _ => None,
        })
        .collect();
    if !mcps.is_empty() {
        let _ = writeln!(out, "  - {} MCP server(s)", mcps.len());
        for name in &mcps {
            let _ = writeln!(out, "      {name}");
        }
    }

    let hooks: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            ImportableItem::Hook {
                event,
                matcher,
                command,
                timeout,
            } => Some((event, matcher, command, timeout)),
            _ => None,
        })
        .collect();
    if !hooks.is_empty() {
        let _ = writeln!(out, "  - {} hook(s)", hooks.len());
        for (event, matcher, command, timeout) in &hooks {
            let timeout = timeout
                .map(|t| format!(" (timeout: {t}s)"))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "      {event} [{}] {command}{timeout}",
                matcher.as_deref().unwrap_or("*")
            );
        }
    }

    for (kind, label) in [(PathKind::Skill, "skill"), (PathKind::Rule, "rule")] {
        let paths: Vec<&String> = items
            .iter()
            .filter_map(|item| match item {
                ImportableItem::PathEntry { kind: k, path } if *k == kind => Some(path),
                _ => None,
            })
            .collect();
        if !paths.is_empty() {
            let _ = writeln!(out, "  - {} extra {label} dir(s)", paths.len());
            for path in &paths {
                let _ = writeln!(out, "      {path}");
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImportPatternMode, ImportPermission, ImportTool};

    #[test]
    fn empty_plan_summary() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            ImportPlan::default().summary(dir.path()),
            "No settings from other coding agents found to import."
        );
    }

    #[test]
    fn summary_lists_items_and_redacts_env_values() {
        let dir = tempfile::tempdir().unwrap();
        let plan = ImportPlan {
            global_items: vec![
                ImportableItem::Permission(ImportPermission {
                    action: ImportRuleAction::Deny,
                    tool: ImportTool::Read,
                    pattern: Some(".env".into()),
                    pattern_mode: ImportPatternMode::Glob,
                }),
                ImportableItem::EnvVar {
                    key: "API_TOKEN".into(),
                    value: "sëcret".into(),
                },
                ImportableItem::Hook {
                    event: "Stop".into(),
                    matcher: None,
                    command: "true".into(),
                    timeout: Some(4),
                },
            ],
            project_items: vec![ImportableItem::PathEntry {
                kind: PathKind::Rule,
                path: "/r".into(),
            }],
        };
        let summary = plan.summary(dir.path());
        assert!(summary.contains("  - 1 permission rule(s) (1 deny)\n      deny Read(.env)\n"));
        assert!(summary.contains("API_TOKEN = <redacted, 6 chars>"));
        assert!(!summary.contains("sëcret"));
        assert!(summary.contains("Stop [*] true (timeout: 4s)"));
        assert!(summary.contains("  - 1 extra rule dir(s)\n      /r\n"));
    }
}
