use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillDescriptor {
    pub name: String,
    pub path: PathBuf,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerDescriptor {
    pub name: String,
    pub transport: String,
    pub endpoint: String,
    pub enabled: bool,
}

#[derive(Debug, Error)]
pub enum IntegrationError {
    #[error("integration I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("integration JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn discover_skills(root: impl AsRef<Path>) -> Result<Vec<SkillDescriptor>, IntegrationError> {
    let root = root.as_ref();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut skills = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest = path.join("SKILL.md");
        if !manifest.exists() {
            continue;
        }
        let content = fs::read_to_string(&manifest)?;
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("unnamed")
            .to_owned();
        let description = content.lines().find_map(|line| {
            line.strip_prefix("description:")
                .map(|value| value.trim().to_owned())
        });
        skills.push(SkillDescriptor {
            name,
            path: manifest,
            description,
        });
    }
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(skills)
}

pub fn parse_mcp_servers(
    value: Option<&str>,
) -> Result<BTreeMap<String, McpServerDescriptor>, IntegrationError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(BTreeMap::new());
    };
    let servers: Vec<McpServerDescriptor> = serde_json::from_str(value)?;
    Ok(servers
        .into_iter()
        .map(|server| (server.name.clone(), server))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{discover_skills, parse_mcp_servers};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn discovers_skill_manifests_and_mcp_descriptors() {
        let dir = tempdir().unwrap();
        let skill = dir.path().join("coding");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "description: Rust coding tools\n").unwrap();
        assert_eq!(discover_skills(dir.path()).unwrap()[0].name, "coding");
        let servers = parse_mcp_servers(Some(
            r#"[{"name":"notion","transport":"stdio","endpoint":"notion-mcp","enabled":true}]"#,
        ))
        .unwrap();
        assert_eq!(servers["notion"].transport, "stdio");
    }
}
