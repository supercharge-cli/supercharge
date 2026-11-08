//! User-authored agents stored as `.supercharge/agents/<name>.md`.
//!
//! The CLI and Studio share this writer. Studio sends the same fields over ACP;
//! the CLI `/agents` form calls it directly. Either way the file is what the
//! next session-mode list and `session/set_mode` resolve.
use crate::config::{AgentDefinition, AgentScope, BuiltinAgentName};
use crate::discovery;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Work-mode ids the session already owns. A custom agent must not reuse them.
const RESERVED_MODE_IDS: &[&str] = &["default", "plan", "ask", "browser_use"];

/// Where the markdown file is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomAgentScope {
    /// `~/.supercharge/agents/` (or `$SUPERCHARGE_HOME/agents`).
    User,
    /// `{cwd}/.supercharge/agents/`.
    Project,
}

/// Fields collected by the CLI form and the Studio form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomAgentDraft {
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub scope: CustomAgentScope,
}

/// One custom agent a chat can switch to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomAgentMode {
    pub id: String,
    pub name: String,
    pub description: String,
    pub scope: AgentScope,
}

/// Validate `name`, write the markdown file, and return the mode id (the file stem).
pub fn write_custom_agent(cwd: &Path, draft: &CustomAgentDraft) -> Result<PathBuf, String> {
    let name = normalize_name(&draft.name)?;
    if draft.instructions.trim().is_empty() {
        return Err("Instructions are required".to_string());
    }
    if draft.instructions.chars().count() > 8_000 {
        return Err("Instructions are limited to 8000 characters".to_string());
    }
    let description = {
        let trimmed = draft.description.trim();
        let text = if trimmed.is_empty() { "Custom agent" } else { trimmed };
        if text.chars().count() > 200 {
            return Err("Description is limited to 200 characters".to_string());
        }
        text.to_string()
    };
    if BuiltinAgentName::from_str(&name).is_ok() {
        return Err(format!("'{name}' is a built-in agent"));
    }
    let dir = agents_dir(draft.scope, cwd)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create agents directory: {e}"))?;
    let path = dir.join(format!("{name}.md"));
    if path.exists() {
        return Err(format!("Agent '{name}' already exists"));
    }
    let body = format!(
        "---\nname: {name}\ndescription: {}\ncustom: true\n---\n\n{}\n",
        yaml_quote(&description),
        draft.instructions.trim()
    );
    std::fs::write(&path, body).map_err(|e| format!("Failed to write agent file: {e}"))?;
    Ok(path)
}

/// Project and user agents this chat can offer next to Agent, Plan, and Ask.
pub fn modes_for_cwd(cwd: &Path) -> Vec<CustomAgentMode> {
    discovery::discover(cwd)
        .into_iter()
        .filter(|def| matches!(def.scope, AgentScope::Project | AgentScope::User))
        .filter(|def| is_custom_agent(def))
        .filter_map(mode_from_definition)
        .collect()
}

/// True only for agents this user created. Skill packs and bundled agents are not custom.
pub fn is_custom_agent(def: &AgentDefinition) -> bool {
    def.source_path.as_ref().is_some_and(|path| {
        std::fs::read_to_string(path)
            .ok()
            .is_some_and(|text| text.lines().take(40).any(|line| line.trim() == "custom: true"))
    })
}

/// Prompt the chat uses for `/new_agent`. The model asks questions, or saves a prompt the user already wrote.
pub fn new_agent_interview(brief: &str, agents_dir: &Path) -> String {
    let supplied = if brief.trim().is_empty() {
        "The user has not written the prompt yet.".to_string()
    } else {
        format!("The user already supplied this. Treat it as the agent prompt unless they say to rewrite it:\n{brief}")
    };
    format!(
        r#"You are helping the user create one custom chat agent. Do not create a skill, a plugin, or a file outside the agents directory.

{supplied}

Ask at most these questions, one at a time, and skip any the user already answered:
1. Should you use their words as the full prompt, or draft the prompt from a short description?
2. Name (lowercase letters, numbers, hyphens).
3. One-line description.
4. Save for every project (user) or only this project (project)?
5. What the agent must always do, and what it must not do.

When you have the name, description, scope, and prompt, write exactly one markdown file with the write tool:
- user scope: {dir}/<name>.md
- project scope: .supercharge/agents/<name>.md in the current project

The file must look like this, with custom set to true:

---
name: <name>
description: "<one line>"
custom: true
---

<the prompt>

After the file is written, tell the user to press Shift+Tab until the banner shows that name. Do not mention skills."#,
        dir = agents_dir.display()
    )
}

fn mode_from_definition(def: AgentDefinition) -> Option<CustomAgentMode> {
    if normalize_name(&def.name).is_err() {
        return None;
    }
    if def.scope == AgentScope::User && BuiltinAgentName::from_str(&def.name).is_ok() {
        return None;
    }
    let description = if def.description.trim().is_empty() {
        "Custom agent".to_string()
    } else {
        def.description
    };
    Some(CustomAgentMode {
        id: def.name.clone(),
        name: def.name,
        description,
        scope: def.scope,
    })
}

fn agents_dir(scope: CustomAgentScope, cwd: &Path) -> Result<PathBuf, String> {
    match scope {
        CustomAgentScope::User => {
            let home = supercharge_dirs::user_supercharge_home()
                .ok_or_else(|| "No home directory for user agents".to_string())?;
            Ok(home.join("agents"))
        }
        CustomAgentScope::Project => Ok(cwd.join(".supercharge").join("agents")),
    }
}

fn normalize_name(raw: &str) -> Result<String, String> {
    let name = raw.trim().to_string();
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok {
        return Err(
            "Name must be lowercase letters, numbers, and hyphens, starting with a letter"
                .to_string(),
        );
    }
    if RESERVED_MODE_IDS.contains(&name.as_str()) {
        return Err(format!("'{name}' is reserved for a built-in work mode"));
    }
    Ok(name)
}

fn yaml_quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cwd() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "supercharge-custom-agent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn project_agent_round_trips_into_session_modes() {
        let cwd = temp_cwd();
        let path = write_custom_agent(
            &cwd,
            &CustomAgentDraft {
                name: "reviewer".into(),
                description: "Reviews diffs".into(),
                instructions: "Comment on the diff. Do not edit files.".into(),
                scope: CustomAgentScope::Project,
            },
        )
        .unwrap();
        assert!(path.ends_with(".supercharge/agents/reviewer.md"));
        let modes = modes_for_cwd(&cwd);
        let reviewer = modes.iter().find(|mode| mode.id == "reviewer").expect("reviewer listed");
        assert_eq!(reviewer.description, "Reviews diffs");
        assert_eq!(reviewer.scope, AgentScope::Project);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn rejects_reserved_and_builtin_names() {
        let cwd = temp_cwd();
        let err = write_custom_agent(
            &cwd,
            &CustomAgentDraft {
                name: "plan".into(),
                description: String::new(),
                instructions: "Think first.".into(),
                scope: CustomAgentScope::Project,
            },
        )
        .unwrap_err();
        assert!(err.contains("reserved"), "{err}");
        let err = write_custom_agent(
            &cwd,
            &CustomAgentDraft {
                name: "explore".into(),
                description: String::new(),
                instructions: "Look around.".into(),
                scope: CustomAgentScope::Project,
            },
        )
        .unwrap_err();
        assert!(err.contains("built-in"), "{err}");
        let _ = std::fs::remove_dir_all(&cwd);
    }
}
