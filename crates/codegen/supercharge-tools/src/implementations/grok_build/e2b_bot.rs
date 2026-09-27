//! Instance-scoped persistent bot management for the E2B Main agent.

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::{
    output::{DynamicOutput, ToolOutput},
    requirements::{Expr, ToolRequirement},
    tool::{ToolKind, ToolNamespace},
};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BotAction {
    List,
    Create,
    Send,
    Get,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BotInput {
    pub action: BotAction,
    /// Name of a new bot (create).
    #[serde(default)]
    pub name: Option<String>,
    /// Preset icon or a validated small image data URL (create).
    #[serde(default)]
    pub icon: Option<String>,
    /// New bot's custom instructions (create).
    #[serde(default)]
    pub instructions: Option<String>,
    /// Absolute default folder inside /home/user/workspace (create).
    #[serde(default)]
    pub default_folder: Option<String>,
    /// Existing recipient bot ID (send).
    #[serde(default)]
    pub bot_id: Option<String>,
    /// The work to delegate to the recipient (send).
    #[serde(default)]
    pub prompt: Option<String>,
    /// ID returned by send, used to retrieve its result (get).
    #[serde(default)]
    pub job_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotOutput {
    pub result: serde_json::Value,
}

impl From<BotOutput> for ToolOutput {
    fn from(value: BotOutput) -> Self {
        ToolOutput::Dynamic(DynamicOutput::from(value.result))
    }
}
impl supercharge_tool_runtime::ToolOutput for BotOutput {}

#[derive(Debug, Default)]
pub struct VectraBotTool;

impl crate::types::tool_metadata::ToolMetadata for VectraBotTool {
    fn kind(&self) -> ToolKind { ToolKind::Other }
    fn tool_namespace(&self) -> ToolNamespace { ToolNamespace::GrokBuild }
    fn description_template(&self) -> &str {
        "Manage the persistent bots of this Vectra E2B instance as Main: list bots, create a named bot with instructions and a folder, send a bot a task, or get a task result. A sent task is queued in the SAME sandbox and returns a job ID; use get to check its attributed response. Bot folders do not provide security isolation."
    }
    fn requires_expr(&self) -> Expr<ToolRequirement> { Expr::True }
}

impl supercharge_tool_runtime::Tool for VectraBotTool {
    type Args = BotInput;
    type Output = BotOutput;
    fn id(&self) -> supercharge_tool_protocol::ToolId {
        supercharge_tool_protocol::ToolId::new("vectra_bot").expect("valid bot tool id")
    }
    fn description(&self, _ctx: &supercharge_tool_runtime::ListToolsContext) -> supercharge_tool_types::ToolDescription {
        supercharge_tool_types::ToolDescription::new("vectra_bot", crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self))
    }
    fn should_list(&self, _ctx: &supercharge_tool_runtime::ListToolsContext) -> bool {
        std::env::var("SUPERCHARGE_RUNTIME_MODE").ok().as_deref() == Some("e2b")
            && std::env::var("VECTRA_BOT_ID").ok().as_deref() == Some("main")
            && std::env::var("VECTRA_BOT_CAPABILITY").is_ok_and(|value| !value.is_empty())
    }
    fn capabilities(&self) -> supercharge_tool_protocol::ToolCapabilities {
        supercharge_tool_protocol::ToolCapabilities {
            is_read_only: false,
            tool_scope: Some(supercharge_tool_protocol::ToolScope::Write),
            ..Default::default()
        }
    }
    async fn run(&self, _ctx: supercharge_tool_runtime::ToolCallContext, input: BotInput) -> Result<BotOutput, supercharge_tool_runtime::ToolError> {
        if std::env::var("SUPERCHARGE_RUNTIME_MODE").ok().as_deref() != Some("e2b") ||
            std::env::var("VECTRA_BOT_ID").ok().as_deref() != Some("main") {
            return Err(supercharge_tool_runtime::ToolError::custom("bot_unavailable", "Only the Main bot in a Supercharge E2B instance can use this tool"));
        }
        let resources = crate::types::tool_metadata::shared_resources(&_ctx)?;
        let session = resources.lock().await.get::<crate::types::resources::OwnerSessionId>().map(|session| session.0.clone());
        if session.as_deref().is_none_or(|session| std::env::var("VECTRA_BOT_SESSION_ID").ok().as_deref() != Some(session)) {
            return Err(supercharge_tool_runtime::ToolError::custom("bot_unavailable", "Persistent bot management is restricted to the Main conversation, not its task subagents"));
        }
        let site = std::env::var("VECTRA_BOT_SITE").map_err(|_| supercharge_tool_runtime::ToolError::custom("bot_unavailable", "Bot service is not configured"))?;
        let instance_id = std::env::var("VECTRA_BOT_INSTANCE_ID").unwrap_or_default();
        let capability = std::env::var("VECTRA_BOT_CAPABILITY").unwrap_or_default();
        if !site.starts_with("https://") || site.contains(['\r', '\n', '@']) ||
            site.len() > 300 || instance_id.is_empty() || capability.len() < 32 {
            return Err(supercharge_tool_runtime::ToolError::custom("bot_unavailable", "Bot service identity is invalid"));
        }
        let (route, body) = match input.action {
            BotAction::List => ("bots", serde_json::json!({"action":"list","instanceId":instance_id})),
            BotAction::Create => {
                let name = input.name.as_deref().unwrap_or("").trim();
                if name.is_empty() || name.chars().count() > 80 {
                    return Err(supercharge_tool_runtime::ToolError::invalid_arguments("Bot name must be 1–80 characters"));
                }
                ("bots", serde_json::json!({"action":"create","instanceId":instance_id,
                    "input":{"name":name,"icon":input.icon,"instructions":input.instructions,
                        "defaultFolder":input.default_folder,"createKey":_ctx.call_id.to_string()}}))
            }
            BotAction::Send => {
                let bot_id = input.bot_id.as_deref().unwrap_or("");
                let prompt = input.prompt.as_deref().unwrap_or("");
                if bot_id.is_empty() || prompt.trim().is_empty() || prompt.len() > 12_000 {
                    return Err(supercharge_tool_runtime::ToolError::invalid_arguments("Recipient bot ID and a prompt under 12,000 characters are required"));
                }
                ("bots/jobs", serde_json::json!({"action":"send","instanceId":instance_id,
                    "botId":bot_id,"prompt":prompt,"requestId":_ctx.call_id.to_string()}))
            }
            BotAction::Get => {
                let job_id = input.job_id.as_deref().unwrap_or("");
                if job_id.is_empty() || job_id.len() > 120 {
                    return Err(supercharge_tool_runtime::ToolError::invalid_arguments("A job ID returned by send is required"));
                }
                ("bots/jobs", serde_json::json!({"action":"get","instanceId":instance_id,"jobId":job_id}))
            }
        };
        let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(90)).build()
            .map_err(|error| supercharge_tool_runtime::ToolError::custom("bot_service", error.to_string()))?;
        let response = client.post(format!("{}/api/internal/{}", site.trim_end_matches('/'), route))
            .header("x-vectra-bot-capability", capability)
            .json(&body).send().await
            .map_err(|error| supercharge_tool_runtime::ToolError::custom("bot_service", error.to_string()))?;
        let status = response.status();
        let value: serde_json::Value = response.json().await
            .map_err(|error| supercharge_tool_runtime::ToolError::custom("bot_service", format!("Bot service returned invalid JSON: {error}")))?;
        if !status.is_success() {
            let message = value.get("error").and_then(serde_json::Value::as_str).unwrap_or("Bot request failed");
            return Err(supercharge_tool_runtime::ToolError::custom("bot_service", message.to_owned()));
        }
        Ok(BotOutput { result: value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_name_is_e2b_specific() {
        assert_eq!(supercharge_tool_runtime::Tool::id(&VectraBotTool).as_str(), "vectra_bot");
    }
}
