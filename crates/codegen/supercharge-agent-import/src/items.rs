use std::collections::HashMap;

use supercharge_config_types::McpServerConfig;

/// Scope for an import operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportScope {
    /// User-level: writes to the user config directory.
    Global,
    /// Project-level: writes to `<repo>/.supercharge/`.
    Project,
}

/// Which `[paths]` field a `PathEntry` populates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// Maps to `[paths] extra_skill_dirs`.
    Skill,
    /// Maps to `[paths] extra_rule_dirs`.
    Rule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportRuleAction {
    Allow,
    Deny,
    Ask,
}

impl ImportRuleAction {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Ask => "ask",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportTool {
    Any,
    Bash,
    Edit,
    Read,
    Grep,
    Mcp,
    WebFetch,
    WebSearch,
    AgentMessage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportPatternMode {
    Glob,
    Domain,
}

/// A permission rule read from another agent's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPermission {
    pub action: ImportRuleAction,
    pub tool: ImportTool,
    pub pattern: Option<String>,
    pub pattern_mode: ImportPatternMode,
}

/// The parts of one settings file that map onto Supercharge config.
#[derive(Debug, Clone, Default)]
pub struct AgentSettingsSnapshot {
    pub permissions: Vec<ImportPermission>,
    pub env: HashMap<String, String>,
}

/// A single item that can be imported.
#[derive(Debug, Clone)]
pub enum ImportableItem {
    Permission(ImportPermission),
    EnvVar {
        key: String,
        value: String,
    },
    McpServer {
        name: String,
        config: Box<McpServerConfig>,
    },
    /// One hook handler: event, optional matcher, and shell command.
    Hook {
        event: String,
        matcher: Option<String>,
        command: String,
        timeout: Option<u64>,
    },
    /// A directory for `[paths] extra_skill_dirs` or `extra_rule_dirs`.
    PathEntry {
        kind: PathKind,
        path: String,
    },
}

/// What would be imported and where.
#[derive(Debug, Clone, Default)]
pub struct ImportPlan {
    pub global_items: Vec<ImportableItem>,
    pub project_items: Vec<ImportableItem>,
}

impl ImportPlan {
    #[must_use]
    pub fn total_items(&self) -> usize {
        self.global_items.len() + self.project_items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.global_items.is_empty() && self.project_items.is_empty()
    }
}

/// Result of applying an import.
#[derive(Debug, Default)]
pub struct ImportResult {
    pub global_count: usize,
    pub project_count: usize,
    pub modified_files: Vec<String>,
}

impl ImportResult {
    #[must_use]
    pub fn total(&self) -> usize {
        self.global_count + self.project_count
    }
}

fn tool_label(tool: ImportTool) -> &'static str {
    match tool {
        ImportTool::Any => "",
        ImportTool::Bash => "Bash",
        ImportTool::Edit => "Edit",
        ImportTool::Read => "Read",
        ImportTool::Grep => "Grep",
        ImportTool::Mcp => "MCPTool",
        ImportTool::WebFetch => "WebFetch",
        ImportTool::WebSearch => "WebSearch",
        ImportTool::AgentMessage => "AgentMessage",
    }
}

/// Format a rule in the compact `Tool(pattern)` form used by `[permission]`.
#[must_use]
pub fn format_rule_string(rule: &ImportPermission) -> String {
    match (rule.pattern.as_deref(), rule.tool) {
        (None, ImportTool::Any) => "*".to_string(),
        (Some(pattern), ImportTool::Any) => pattern.to_string(),
        (None, tool) => tool_label(tool).to_string(),
        (Some(pattern), tool) => {
            let pattern = match rule.pattern_mode {
                ImportPatternMode::Domain => format!("domain:{pattern}"),
                ImportPatternMode::Glob => pattern.to_string(),
            };
            format!("{}({pattern})", tool_label(tool))
        }
    }
}

#[cfg(test)]
#[path = "items_tests.rs"]
mod tests;
