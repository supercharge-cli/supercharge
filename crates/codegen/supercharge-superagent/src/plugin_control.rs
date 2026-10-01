use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginBridgeTool {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginBridgeStatus {
    pub configured: bool,
    pub revision: Option<u64>,
    pub session_id: Option<String>,
    pub agent_session_id: Option<String>,
    pub plugin_id: Option<String>,
    pub server_id: Option<String>,
    pub release_digest: Option<String>,
    pub generation: Option<String>,
    pub server_name: Option<String>,
    pub policy_expires_at: Option<String>,
    pub status: String,
    pub last_error: Option<String>,
    #[serde(default)]
    pub tools: Vec<PluginBridgeTool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginMcpDescriptor {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<PluginMcpEnv>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginMcpEnv {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginControlResult {
    #[serde(default)]
    pub service: Option<Value>,
    #[serde(default)]
    pub inventory: Option<Value>,
    #[serde(default)]
    pub bindings: Option<Value>,
    #[serde(default)]
    pub binding: Option<PluginBridgeStatus>,
    #[serde(default)]
    pub descriptor: Option<PluginMcpDescriptor>,
    #[serde(default)]
    pub agent_uid: Option<u32>,
    #[serde(default)]
    pub plugin: Option<Value>,
    #[serde(default)]
    pub valid: Option<bool>,
    #[serde(default)]
    pub manifest: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ControllerEnvelope {
    ok: bool,
    result: Option<PluginControlResult>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotIdentity<'a> {
    pub id: &'a str,
    pub revision: u64,
    pub default_folder: &'a str,
}

fn controller_path() -> PathBuf {
    std::env::var_os("SUPERAGENT_PLUGIN_CONTROL")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/vectra/plugin-api/control.mjs"))
}

fn controller_node() -> PathBuf {
    std::env::var_os("SUPERAGENT_PLUGIN_API_NODE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/vectra/plugin-api/node/bin/node"))
}

pub fn available() -> bool {
    cfg!(target_os = "linux")
        && crate::browser::is_e2b_runtime()
        && controller_path().is_file()
        && controller_node().is_file()
}

pub async fn request(
    instance_id: &str,
    sandbox_id: &str,
    bot: BotIdentity<'_>,
    action: &str,
    body: Value,
) -> Result<PluginControlResult, String> {
    if !available() {
        return Err("Managed MCP requires the Supercharge E2B runtime with its isolated Plugin API controller".into());
    }
    if instance_id.is_empty() || sandbox_id.is_empty() {
        return Err("Sandbox target identity is unavailable".into());
    }
    let mut payload = body;
    let object = payload
        .as_object_mut()
        .ok_or("Plugin control payload must be an object")?;
    let capability_file = std::env::var_os("SUPERAGENT_PLUGIN_CONTROL_CAPABILITY_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/user/.superagent/plugin-control.capability"));
    let capability = std::fs::read_to_string(&capability_file)
        .map_err(|_| "Plugin controller capability is unavailable")?;
    let capability = capability.trim();
    if capability.len() < 32 || capability.len() > 256 {
        return Err("Plugin controller capability is invalid".into());
    }
    object.insert("action".into(), Value::String(action.to_string()));
    object.insert("capability".into(), Value::String(capability.to_string()));
    object.insert("instanceId".into(), Value::String(instance_id.to_string()));
    object.insert("sandboxId".into(), Value::String(sandbox_id.to_string()));
    object.insert(
        "bot".into(),
        serde_json::to_value(bot).map_err(|error| error.to_string())?,
    );
    let encoded = serde_json::to_vec(&payload).map_err(|error| error.to_string())?;
    if encoded.len() > 512 * 1024 {
        return Err("Plugin control request exceeds 512 KiB".into());
    }

    // The daemon stays unprivileged; the template's sudoers allowlist permits
    // exactly this fixed node + controller invocation as root. When the daemon
    // already runs as root (tests / alternate deployments), exec directly.
    let is_root = unsafe { libc::geteuid() } == 0;
    let node = controller_node();
    let controller = controller_path();
    let mut command = if is_root {
        let mut command = tokio::process::Command::new(node);
        command.arg(controller);
        command
    } else {
        let mut command = tokio::process::Command::new("sudo");
        command.arg("-n").arg(node).arg(controller);
        command
    };
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Plugin controller could not start: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or("Plugin controller stdin is unavailable")?;
    stdin
        .write_all(&encoded)
        .await
        .map_err(|error| format!("Plugin controller input failed: {error}"))?;
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(150), child.wait_with_output())
        .await
        .map_err(|_| "Plugin controller timed out")?
        .map_err(|error| format!("Plugin controller failed: {error}"))?;
    if output.stdout.len() > MAX_RESPONSE_BYTES {
        return Err("Plugin controller response exceeds 1 MiB".into());
    }
    let envelope: ControllerEnvelope = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Plugin controller returned an invalid response")?;
    if !output.status.success() || !envelope.ok {
        return Err(envelope
            .error
            .unwrap_or_else(|| "Plugin controller failed".into()));
    }
    envelope
        .result
        .ok_or_else(|| "Plugin controller returned no result".into())
}

pub fn descriptor_to_acp(descriptor: &PluginMcpDescriptor) -> Result<Value, String> {
    if descriptor.name.is_empty()
        || descriptor.command.is_empty()
        || descriptor.name.len() > 64
        || descriptor.args.len() > 64
        || descriptor.env.len() > 64
    {
        return Err("Managed MCP bridge descriptor is invalid".into());
    }
    if !Path::new(&descriptor.command).is_absolute() {
        return Err("Managed MCP bridge command must be absolute".into());
    }
    Ok(serde_json::json!({
        "name": descriptor.name,
        "command": descriptor.command,
        "args": descriptor.args,
        "env": descriptor.env,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_conversion_is_exact_and_rejects_relative_commands() {
        let valid = PluginMcpDescriptor {
            name: "vectra-plugin-api-abcd".into(),
            command: "/usr/bin/node".into(),
            args: vec!["/opt/vectra/plugin-api/plugin-bridge-0.1.0.mjs".into()],
            env: vec![PluginMcpEnv {
                name: "SUPERCHARGE_PLUGIN_BRIDGE_BINDING".into(),
                value: "/run/vectra-plugin-bindings/bot/session/generation/binding.json".into(),
            }],
        };
        assert_eq!(descriptor_to_acp(&valid).unwrap()["name"], valid.name);
        let mut invalid = valid;
        invalid.command = "node".into();
        assert!(descriptor_to_acp(&invalid).is_err());
    }
}
