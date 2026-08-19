use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillDescriptor {
    pub name: String,
    pub path: PathBuf,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoadedSkill {
    pub descriptor: SkillDescriptor,
    pub instructions: String,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct SkillRegistry {
    root: PathBuf,
    max_skill_bytes: usize,
}

impl SkillRegistry {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            max_skill_bytes: 512 * 1024,
        }
    }

    pub fn discover(&self) -> Result<Vec<SkillDescriptor>, IntegrationError> {
        discover_skills(&self.root)
    }

    #[allow(clippy::disallowed_methods)]
    pub fn load(&self, name: &str) -> Result<LoadedSkill, IntegrationError> {
        if name.is_empty()
            || name.contains('/')
            || name.contains('\\')
            || name == "."
            || name == ".."
        {
            return Err(IntegrationError::Invalid("invalid skill name".to_owned()));
        }
        let root = fs::canonicalize(&self.root)?;
        let manifest = root.join(name).join("SKILL.md");
        let canonical = fs::canonicalize(&manifest)?;
        if !canonical.starts_with(&root) {
            return Err(IntegrationError::Invalid(
                "skill path escapes configured root".to_owned(),
            ));
        }
        let metadata = fs::metadata(&canonical)?;
        if metadata.len() as usize > self.max_skill_bytes {
            return Err(IntegrationError::Invalid(
                "skill manifest exceeds size limit".to_owned(),
            ));
        }
        let instructions = fs::read_to_string(&canonical)?;
        let description = instructions.lines().find_map(|line| {
            line.strip_prefix("description:")
                .map(|value| value.trim().to_owned())
        });
        let digest = Sha256::digest(instructions.as_bytes());
        Ok(LoadedSkill {
            descriptor: SkillDescriptor {
                name: name.to_owned(),
                path: canonical,
                description,
            },
            instructions,
            sha256: format!("sha256:{digest:x}"),
        })
    }
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
    #[error("invalid integration configuration: {0}")]
    Invalid(String),
    #[error("integration HTTP request failed: {0}")]
    Http(String),
    #[error("integration protocol response was invalid: {0}")]
    Protocol(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpToolSummary {
    pub name: String,
    pub description: Option<String>,
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
    let mut parsed = BTreeMap::new();
    for server in servers {
        validate_mcp_server(&server)?;
        parsed.insert(server.name.clone(), server);
    }
    Ok(parsed)
}

pub async fn discover_http_tools(
    server: &McpServerDescriptor,
) -> Result<Vec<McpToolSummary>, IntegrationError> {
    validate_mcp_server(server)?;
    if !server.enabled {
        return Ok(Vec::new());
    }
    if !matches!(server.transport.as_str(), "http" | "sse") {
        return Err(IntegrationError::Invalid(
            "only HTTP and SSE MCP discovery is enabled".to_owned(),
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|error| IntegrationError::Http(error.to_string()))?;
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "superagent", "version": env!("CARGO_PKG_VERSION")}
        }
    });
    let init = client
        .post(&server.endpoint)
        .header("Accept", "application/json, text/event-stream")
        .json(&initialize)
        .send()
        .await
        .map_err(|error| IntegrationError::Http(error.to_string()))?;
    if !init.status().is_success() {
        return Err(IntegrationError::Http(format!(
            "MCP initialize returned HTTP {}",
            init.status()
        )));
    }
    let _: Value = init
        .json()
        .await
        .map_err(|error| IntegrationError::Protocol(error.to_string()))?;
    let tools_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    let response = client
        .post(&server.endpoint)
        .header("Accept", "application/json, text/event-stream")
        .json(&tools_request)
        .send()
        .await
        .map_err(|error| IntegrationError::Http(error.to_string()))?;
    if !response.status().is_success() {
        return Err(IntegrationError::Http(format!(
            "MCP tools/list returned HTTP {}",
            response.status()
        )));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|error| IntegrationError::Protocol(error.to_string()))?;
    let tools = body
        .get("result")
        .and_then(|value| value.get("tools"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            IntegrationError::Protocol("MCP response has no result.tools array".to_owned())
        })?;
    tools
        .iter()
        .map(|tool| {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| IntegrationError::Protocol("MCP tool has no name".to_owned()))?;
            Ok(McpToolSummary {
                name: name.to_owned(),
                description: tool
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

pub fn validate_mcp_server(server: &McpServerDescriptor) -> Result<(), IntegrationError> {
    if server.name.trim().is_empty() || server.name.contains('/') || server.name.contains('\\') {
        return Err(IntegrationError::Invalid(
            "MCP server name is invalid".to_owned(),
        ));
    }
    if !matches!(server.transport.as_str(), "stdio" | "http" | "sse") {
        return Err(IntegrationError::Invalid(format!(
            "unsupported MCP transport: {}",
            server.transport
        )));
    }
    if server.endpoint.trim().is_empty() {
        return Err(IntegrationError::Invalid(
            "MCP endpoint is empty".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SkillRegistry, discover_skills, parse_mcp_servers};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn discovers_skill_manifests_and_mcp_descriptors() {
        let dir = tempdir().unwrap();
        let skill = dir.path().join("coding");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "description: Rust coding tools\n").unwrap();
        assert_eq!(discover_skills(dir.path()).unwrap()[0].name, "coding");
        let loaded = SkillRegistry::new(dir.path()).load("coding").unwrap();
        assert!(loaded.instructions.contains("Rust coding tools"));
        assert!(loaded.sha256.starts_with("sha256:"));
        let servers = parse_mcp_servers(Some(
            r#"[{"name":"notion","transport":"stdio","endpoint":"notion-mcp","enabled":true}]"#,
        ))
        .unwrap();
        assert_eq!(servers["notion"].transport, "stdio");
        assert!(
            parse_mcp_servers(Some(
                r#"[{"name":"bad","transport":"grpc","endpoint":"x","enabled":true}]"#,
            ))
            .is_err()
        );
    }
}
