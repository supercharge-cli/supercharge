use super::*;

fn rule(tool: ImportTool, pattern: Option<&str>, mode: ImportPatternMode) -> ImportPermission {
    ImportPermission {
        action: ImportRuleAction::Allow,
        tool,
        pattern: pattern.map(str::to_string),
        pattern_mode: mode,
    }
}

#[test]
fn format_rule_tool_with_pattern() {
    let r = rule(
        ImportTool::Bash,
        Some("npm run build"),
        ImportPatternMode::Glob,
    );
    assert_eq!(format_rule_string(&r), "Bash(npm run build)");
}

#[test]
fn format_rule_bare_tool_name() {
    let r = rule(ImportTool::Bash, None, ImportPatternMode::Glob);
    assert_eq!(format_rule_string(&r), "Bash");
}

#[test]
fn format_rule_any_without_pattern_is_star() {
    let r = rule(ImportTool::Any, None, ImportPatternMode::Glob);
    assert_eq!(format_rule_string(&r), "*");
}

#[test]
fn format_rule_any_with_pattern_is_the_pattern() {
    let r = rule(ImportTool::Any, Some("src/**"), ImportPatternMode::Glob);
    assert_eq!(format_rule_string(&r), "src/**");
}

#[test]
fn format_rule_domain_pattern() {
    let r = rule(
        ImportTool::WebFetch,
        Some("example.com"),
        ImportPatternMode::Domain,
    );
    assert_eq!(format_rule_string(&r), "WebFetch(domain:example.com)");
}

#[test]
fn format_rule_covers_every_tool() {
    for (tool, label) in [
        (ImportTool::Edit, "Edit"),
        (ImportTool::Read, "Read"),
        (ImportTool::Grep, "Grep"),
        (ImportTool::Mcp, "MCPTool"),
        (ImportTool::WebSearch, "WebSearch"),
        (ImportTool::AgentMessage, "AgentMessage"),
    ] {
        assert_eq!(
            format_rule_string(&rule(tool, None, ImportPatternMode::Glob)),
            label
        );
    }
}

#[test]
fn plan_and_result_totals() {
    let mut plan = ImportPlan::default();
    assert!(plan.is_empty());
    plan.project_items.push(ImportableItem::EnvVar {
        key: "A".into(),
        value: "b".into(),
    });
    assert!(!plan.is_empty());
    assert_eq!(plan.total_items(), 1);
    let result = ImportResult {
        global_count: 2,
        project_count: 3,
        modified_files: Vec::new(),
    };
    assert_eq!(result.total(), 5);
}
