use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use tokio::{io::AsyncWriteExt, process::Command};

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
        let digest_text = format!("sha256:{digest:x}");
        if std::env::var("SUPERAGENT_SKILL_REQUIRE_TRUST")
            .ok()
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
        {
            let trust_file = canonical.with_extension("sha256");
            let trusted = fs::read_to_string(&trust_file)
                .ok()
                .is_some_and(|value| value.split_whitespace().any(|item| item == digest_text));
            if !trusted {
                return Err(IntegrationError::Invalid(
                    "skill manifest is not trusted by its adjacent .sha256 file".to_owned(),
                ));
            }
        }
        Ok(LoadedSkill {
            descriptor: SkillDescriptor {
                name: name.to_owned(),
                path: canonical,
                description,
            },
            instructions,
            sha256: digest_text,
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

pub async fn invoke_http_tool(
    server: &McpServerDescriptor,
    tool_name: &str,
    arguments: Value,
    timeout_seconds: Option<u64>,
) -> Result<Value, IntegrationError> {
    validate_mcp_server(server)?;
    if !server.enabled {
        return Err(IntegrationError::Invalid(
            "MCP server is disabled".to_owned(),
        ));
    }
    if !matches!(server.transport.as_str(), "http" | "sse") {
        return Err(IntegrationError::Invalid(
            "only HTTP and SSE MCP invocation is enabled".to_owned(),
        ));
    }
    if tool_name.trim().is_empty()
        || tool_name.len() > 128
        || tool_name.chars().any(char::is_control)
    {
        return Err(IntegrationError::Invalid(
            "MCP tool name is invalid".to_owned(),
        ));
    }
    let encoded_arguments = serde_json::to_vec(&arguments)?;
    if encoded_arguments.len() > 256 * 1024 {
        return Err(IntegrationError::Invalid(
            "MCP tool arguments exceed the 256 KiB limit".to_owned(),
        ));
    }
    let timeout = std::time::Duration::from_secs(timeout_seconds.unwrap_or(30).clamp(1, 120));
    let client = reqwest::Client::builder()
        .timeout(timeout)
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
    let initialize_response = client
        .post(&server.endpoint)
        .header("Accept", "application/json, text/event-stream")
        .json(&initialize)
        .send()
        .await
        .map_err(|error| IntegrationError::Http(error.to_string()))?;
    if !initialize_response.status().is_success() {
        return Err(IntegrationError::Http(format!(
            "MCP initialize returned HTTP {}",
            initialize_response.status()
        )));
    }
    let session_id = initialize_response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let initialize_body = parse_mcp_response(initialize_response).await?;
    if let Some(error) = initialize_body.get("error") {
        return Err(IntegrationError::Protocol(format!(
            "MCP initialize failed: {error}"
        )));
    }
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": tool_name, "arguments": arguments}
    });
    let mut call = client
        .post(&server.endpoint)
        .header("Accept", "application/json, text/event-stream")
        .json(&request);
    if let Some(session_id) = session_id {
        call = call.header("Mcp-Session-Id", session_id);
    }
    let response = call
        .send()
        .await
        .map_err(|error| IntegrationError::Http(error.to_string()))?;
    if !response.status().is_success() {
        return Err(IntegrationError::Http(format!(
            "MCP tools/call returned HTTP {}",
            response.status()
        )));
    }
    let body = parse_mcp_response(response).await?;
    if let Some(error) = body.get("error") {
        return Err(IntegrationError::Protocol(format!(
            "MCP tool call failed: {error}"
        )));
    }
    Ok(body.get("result").cloned().unwrap_or(body))
}

fn stdio_allowed(endpoint: &str) -> bool {
    std::env::var("SUPERAGENT_MCP_STDIO_ALLOWLIST")
        .ok()
        .map(|value| value.split(',').map(str::trim).any(|item| item == endpoint))
        .unwrap_or(false)
}

#[allow(clippy::disallowed_methods)]
async fn stdio_rpc(
    server: &McpServerDescriptor,
    requests: &[Value],
    timeout: std::time::Duration,
) -> Result<Vec<Value>, IntegrationError> {
    if !stdio_allowed(&server.endpoint) {
        return Err(IntegrationError::Invalid(
            "stdio MCP endpoint is not present in SUPERAGENT_MCP_STDIO_ALLOWLIST".to_owned(),
        ));
    }
    let mut child = Command::new(&server.endpoint)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| IntegrationError::Http(format!("stdio MCP spawn failed: {error}")))?;
    let input = requests
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()?
        .join("\\n");
    if input.len() > 512 * 1024 {
        return Err(IntegrationError::Invalid(
            "stdio MCP request exceeds the 512 KiB limit".to_owned(),
        ));
    }
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(format!("{input}\n").as_bytes())
            .await
            .map_err(|error| IntegrationError::Http(error.to_string()))?;
    }
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| IntegrationError::Http("stdio MCP process timed out".to_owned()))?
        .map_err(|error| IntegrationError::Http(error.to_string()))?;
    if output.stdout.len() > 512 * 1024 {
        return Err(IntegrationError::Invalid(
            "stdio MCP output exceeds the 512 KiB limit".to_owned(),
        ));
    }
    output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_slice(line)
                .map_err(|error| IntegrationError::Protocol(error.to_string()))
        })
        .collect()
}

pub async fn invoke_stdio_tool(
    server: &McpServerDescriptor,
    tool_name: &str,
    arguments: Value,
    timeout_seconds: Option<u64>,
) -> Result<Value, IntegrationError> {
    validate_mcp_server(server)?;
    if !server.enabled || server.transport != "stdio" {
        return Err(IntegrationError::Invalid(
            "stdio MCP server is disabled or has the wrong transport".to_owned(),
        ));
    }
    if tool_name.trim().is_empty()
        || tool_name.len() > 128
        || tool_name.chars().any(char::is_control)
    {
        return Err(IntegrationError::Invalid(
            "MCP tool name is invalid".to_owned(),
        ));
    }
    let encoded_arguments = serde_json::to_vec(&arguments)?;
    if encoded_arguments.len() > 256 * 1024 {
        return Err(IntegrationError::Invalid(
            "MCP tool arguments exceed the 256 KiB limit".to_owned(),
        ));
    }
    let requests = vec![
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"superagent","version":env!("CARGO_PKG_VERSION")}}}),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":tool_name,"arguments":arguments}}),
    ];
    let responses = stdio_rpc(
        server,
        &requests,
        std::time::Duration::from_secs(timeout_seconds.unwrap_or(30).clamp(1, 120)),
    )
    .await?;
    let body = responses
        .into_iter()
        .last()
        .ok_or_else(|| IntegrationError::Protocol("stdio MCP returned no response".to_owned()))?;
    if let Some(error) = body.get("error") {
        return Err(IntegrationError::Protocol(format!(
            "MCP tool call failed: {error}"
        )));
    }
    Ok(body.get("result").cloned().unwrap_or(body))
}

pub async fn discover_stdio_tools(
    server: &McpServerDescriptor,
) -> Result<Vec<McpToolSummary>, IntegrationError> {
    validate_mcp_server(server)?;
    if !server.enabled || server.transport != "stdio" {
        return Ok(Vec::new());
    }
    let requests = vec![
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"superagent","version":env!("CARGO_PKG_VERSION")}}}),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    ];
    let responses = stdio_rpc(server, &requests, std::time::Duration::from_secs(30)).await?;
    let body = responses
        .into_iter()
        .last()
        .ok_or_else(|| IntegrationError::Protocol("stdio MCP returned no response".to_owned()))?;
    let tools = body
        .get("result")
        .and_then(|result| result.get("tools"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            IntegrationError::Protocol("MCP response has no result.tools array".to_owned())
        })?;
    tools
        .iter()
        .map(|tool| {
            Ok(McpToolSummary {
                name: tool
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| IntegrationError::Protocol("MCP tool has no name".to_owned()))?
                    .to_owned(),
                description: tool
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

async fn parse_mcp_response(response: reqwest::Response) -> Result<Value, IntegrationError> {
    let is_sse = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    let text = response
        .text()
        .await
        .map_err(|error| IntegrationError::Protocol(error.to_string()))?;
    if !is_sse {
        return serde_json::from_str(&text)
            .map_err(|error| IntegrationError::Protocol(error.to_string()));
    }
    let mut last = None;
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        last = Some(
            serde_json::from_str(data)
                .map_err(|error| IntegrationError::Protocol(error.to_string()))?,
        );
    }
    last.ok_or_else(|| {
        IntegrationError::Protocol("MCP SSE response contained no data event".to_owned())
    })
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
    use super::{
        McpServerDescriptor, SkillRegistry, discover_skills, invoke_http_tool, parse_mcp_servers,
    };
    use httpmock::prelude::*;
    use serde_json::json;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn invokes_http_tool_and_forwards_mcp_session() {
        let server = MockServer::start_async().await;
        let initialize = server
            .mock_async(|when, then| {
                when.method(POST).path("/").json_body(json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-06-18",
                        "capabilities": {},
                        "clientInfo": {"name": "superagent", "version": env!("CARGO_PKG_VERSION")}
                    }
                }));
                then.status(200)
                    .header("content-type", "application/json")
                    .header("mcp-session-id", "session-1")
                    .json_body(json!({"jsonrpc": "2.0", "id": 1, "result": {}}));
            })
            .await;
        let call = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/")
                    .header("mcp-session-id", "session-1")
                    .json_body(json!({
                        "jsonrpc": "2.0",
                        "id": 2,
                        "method": "tools/call",
                        "params": {"name": "echo", "arguments": {"text": "hello"}}
                    }));
                then.status(200)
                    .header("content-type", "application/json")
                    .json_body(json!({
                        "jsonrpc": "2.0",
                        "id": 2,
                        "result": {"content": [{"type": "text", "text": "hello"}]}
                    }));
            })
            .await;
        let result = invoke_http_tool(
            &McpServerDescriptor {
                name: "mock".to_owned(),
                transport: "http".to_owned(),
                endpoint: server.url("/"),
                enabled: true,
            },
            "echo",
            json!({"text": "hello"}),
            Some(5),
        )
        .await
        .unwrap();
        assert_eq!(result["content"][0]["text"], "hello");
        initialize.assert_async().await;
        call.assert_async().await;
    }

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
