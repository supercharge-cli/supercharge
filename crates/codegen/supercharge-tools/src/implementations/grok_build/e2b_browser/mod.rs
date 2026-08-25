//! Native Playwright browser tools for the E2B-only Supercharge runtime.
//!
//! The tools never start a browser. They can talk only to the localhost browser
//! sidecar started by the E2B template, so a normal local CLI has neither the
//! tool schema nor a browser process.

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::output::{DynamicOutput, ToolOutput};
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

const MAX_TEXT_CHARS: usize = 16_000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserNavigateInput {
    /// HTTP or HTTPS URL to open in the shared persistent browser context.
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct BrowserSnapshotInput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserClickInput {
    /// Element reference returned by browser_snapshot, for example `e3`.
    pub r#ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserFillInput {
    /// Element reference returned by browser_snapshot.
    pub r#ref: String,
    /// Replacement text for the input or textarea.
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserPressInput {
    /// Keyboard key, for example Enter, Tab, or Control+A.
    pub key: String,
    /// Optional element reference to focus before pressing the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct BrowserScreenshotInput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserHandoffInput {
    /// `start` asks the authenticated app user to take control through noVNC;
    /// `stop` returns control to the agent.
    pub action: BrowserHandoffAction,
    /// Optional expiry in minutes for a started handoff. Range: 1-30.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minutes: Option<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserHandoffAction {
    Start,
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserToolOutput {
    pub result: serde_json::Value,
}

impl From<BrowserToolOutput> for ToolOutput {
    fn from(value: BrowserToolOutput) -> Self {
        ToolOutput::Dynamic(DynamicOutput::from(value.result))
    }
}

impl supercharge_tool_runtime::ToolOutput for BrowserToolOutput {}

fn e2b_enabled() -> bool {
    std::env::var("SUPERCHARGE_RUNTIME_MODE")
        .is_ok_and(|value| value.eq_ignore_ascii_case("e2b"))
}

fn sidecar_base_url() -> Result<String, supercharge_tool_runtime::ToolError> {
    if !e2b_enabled() {
        return Err(supercharge_tool_runtime::ToolError::custom(
            "browser_unavailable",
            "browser tools are available only in an E2B Supercharge runtime",
        ));
    }
    let port = std::env::var("SUPERAGENT_BROWSER_SIDECAR_PORT").unwrap_or_else(|_| "9224".to_owned());
    let port: u16 = port.parse().map_err(|_| {
        supercharge_tool_runtime::ToolError::custom(
            "browser_unavailable",
            "SUPERAGENT_BROWSER_SIDECAR_PORT is invalid",
        )
    })?;
    Ok(format!("http://127.0.0.1:{port}"))
}

async fn call_sidecar(
    method: reqwest::Method,
    path: &str,
    payload: Option<serde_json::Value>,
) -> Result<BrowserToolOutput, supercharge_tool_runtime::ToolError> {
    let url = format!("{}{}", sidecar_base_url()?, path);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| supercharge_tool_runtime::ToolError::custom("browser_sidecar", error.to_string()))?;
    let request = client.request(method, url);
    let response = match payload {
        Some(value) => request.json(&value).send().await,
        None => request.send().await,
    }
    .map_err(|error| {
        supercharge_tool_runtime::ToolError::custom("browser_sidecar", format!(
            "E2B browser sidecar is unavailable: {error}"
        ))
    })?;
    let status = response.status();
    let value = response.json::<serde_json::Value>().await.map_err(|error| {
        supercharge_tool_runtime::ToolError::custom("browser_sidecar", format!(
            "E2B browser sidecar returned invalid JSON: {error}"
        ))
    })?;
    if !status.is_success() || value.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
        let detail = value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("E2B browser sidecar request failed");
        return Err(supercharge_tool_runtime::ToolError::custom("browser_sidecar", detail.to_owned()));
    }
    Ok(BrowserToolOutput { result: value })
}

fn valid_text(value: &str, label: &str, max: usize) -> Result<(), supercharge_tool_runtime::ToolError> {
    if value.trim().is_empty() || value.len() > max || value.contains('\0') {
        return Err(supercharge_tool_runtime::ToolError::invalid_arguments(format!(
            "{label} is empty, invalid, or too long"
        )));
    }
    Ok(())
}

macro_rules! browser_tool {
    ($name:ident, $input:ty, $id:literal, $description:literal, $method:expr, $path:literal, $payload:expr, $readonly:expr) => {
        #[derive(Debug, Default)]
        pub struct $name;

        impl crate::types::tool_metadata::ToolMetadata for $name {
            fn kind(&self) -> ToolKind { ToolKind::Other }
            fn tool_namespace(&self) -> ToolNamespace { ToolNamespace::GrokBuild }
            fn description_template(&self) -> &str { $description }
            fn requires_expr(&self) -> Expr<ToolRequirement> { Expr::True }
        }

        impl supercharge_tool_runtime::Tool for $name {
            type Args = $input;
            type Output = BrowserToolOutput;
            fn id(&self) -> supercharge_tool_protocol::ToolId {
                supercharge_tool_protocol::ToolId::new($id).expect("valid browser tool id")
            }
            fn description(&self, _ctx: &supercharge_tool_runtime::ListToolsContext) -> supercharge_tool_types::ToolDescription {
                supercharge_tool_types::ToolDescription::new($id, crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self))
            }
            fn capabilities(&self) -> supercharge_tool_protocol::ToolCapabilities {
                supercharge_tool_protocol::ToolCapabilities {
                    is_read_only: $readonly,
                    tool_scope: Some(if $readonly { supercharge_tool_protocol::ToolScope::Read } else { supercharge_tool_protocol::ToolScope::Write }),
                    ..Default::default()
                }
            }
            async fn run(&self, _ctx: supercharge_tool_runtime::ToolCallContext, input: $input) -> Result<BrowserToolOutput, supercharge_tool_runtime::ToolError> {
                let payload: Result<Option<serde_json::Value>, supercharge_tool_runtime::ToolError> = $payload(input);
                call_sidecar($method, $path, payload?).await
            }
        }
    };
}

browser_tool!(
    BrowserNavigateTool,
    BrowserNavigateInput,
    "browser_navigate",
    "Open an HTTP or HTTPS URL in the shared E2B Playwright browser. Use browser_snapshot after navigation to inspect accessible elements.",
    reqwest::Method::POST,
    "/navigate",
    |input: BrowserNavigateInput| { valid_text(&input.url, "url", 2_048)?; Ok(Some(serde_json::json!({ "url": input.url }))) },
    false
);
browser_tool!(
    BrowserSnapshotTool,
    BrowserSnapshotInput,
    "browser_snapshot",
    "Read the shared E2B browser accessibility snapshot and element references. Use refs from this result with browser_click, browser_fill, or browser_press. Prefer this tool before screenshots.",
    reqwest::Method::GET,
    "/snapshot",
    |_input: BrowserSnapshotInput| Ok::<Option<serde_json::Value>, supercharge_tool_runtime::ToolError>(None),
    true
);
browser_tool!(
    BrowserClickTool,
    BrowserClickInput,
    "browser_click",
    "Click an element reference from browser_snapshot in the shared E2B browser.",
    reqwest::Method::POST,
    "/click",
    |input: BrowserClickInput| { valid_text(&input.r#ref, "ref", 64)?; Ok(Some(serde_json::json!({ "ref": input.r#ref }))) },
    false
);
browser_tool!(
    BrowserFillTool,
    BrowserFillInput,
    "browser_fill",
    "Replace the value of an input or textarea reference from browser_snapshot.",
    reqwest::Method::POST,
    "/fill",
    |input: BrowserFillInput| { valid_text(&input.r#ref, "ref", 64)?; valid_text(&input.text, "text", MAX_TEXT_CHARS)?; Ok(Some(serde_json::json!({ "ref": input.r#ref, "text": input.text }))) },
    false
);
browser_tool!(
    BrowserPressTool,
    BrowserPressInput,
    "browser_press",
    "Press a keyboard key in the shared E2B browser, optionally after focusing an element reference.",
    reqwest::Method::POST,
    "/press",
    |input: BrowserPressInput| { valid_text(&input.key, "key", 80)?; if let Some(reference) = &input.r#ref { valid_text(reference, "ref", 64)?; } Ok(Some(serde_json::json!({ "key": input.key, "ref": input.r#ref }))) },
    false
);
browser_tool!(
    BrowserScreenshotTool,
    BrowserScreenshotInput,
    "browser_screenshot",
    "Capture a JPEG screenshot of the shared E2B browser when visual proof is necessary. Prefer browser_snapshot for normal interaction.",
    reqwest::Method::GET,
    "/screenshot",
    |_input: BrowserScreenshotInput| Ok::<Option<serde_json::Value>, supercharge_tool_runtime::ToolError>(None),
    true
);
// BrowserHandoff needs a variable endpoint, so implement it explicitly rather than
// teaching the shared macro a special case.
#[derive(Debug, Default)]
pub struct BrowserHandoffTool;

impl crate::types::tool_metadata::ToolMetadata for BrowserHandoffTool {
    fn kind(&self) -> ToolKind { ToolKind::Other }
    fn tool_namespace(&self) -> ToolNamespace { ToolNamespace::GrokBuild }
    fn description_template(&self) -> &str { "Request an explicit authenticated user handoff for login, MFA, OAuth, CAPTCHA, or other user-only browser actions. Starting a handoff enables the app browser-tab noVNC view for up to 30 minutes; stopping returns control to the agent." }
    fn requires_expr(&self) -> Expr<ToolRequirement> { Expr::True }
}

impl supercharge_tool_runtime::Tool for BrowserHandoffTool {
    type Args = BrowserHandoffInput;
    type Output = BrowserToolOutput;
    fn id(&self) -> supercharge_tool_protocol::ToolId { supercharge_tool_protocol::ToolId::new("browser_handoff").expect("valid browser tool id") }
    fn description(&self, _ctx: &supercharge_tool_runtime::ListToolsContext) -> supercharge_tool_types::ToolDescription { supercharge_tool_types::ToolDescription::new("browser_handoff", crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self)) }
    fn capabilities(&self) -> supercharge_tool_protocol::ToolCapabilities { supercharge_tool_protocol::ToolCapabilities { is_read_only: false, tool_scope: Some(supercharge_tool_protocol::ToolScope::Write), ..Default::default() } }
    async fn run(&self, _ctx: supercharge_tool_runtime::ToolCallContext, input: BrowserHandoffInput) -> Result<BrowserToolOutput, supercharge_tool_runtime::ToolError> {
        let (path, body) = match input.action { BrowserHandoffAction::Start => ("/handoff/start", serde_json::json!({ "minutes": input.minutes.unwrap_or(10).clamp(1, 30) })), BrowserHandoffAction::Stop => ("/handoff/stop", serde_json::json!({})) };
        call_sidecar(reqwest::Method::POST, path, Some(body)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_tool_ids_are_stable_and_distinct() {
        assert_eq!(supercharge_tool_runtime::Tool::id(&BrowserSnapshotTool).as_str(), "browser_snapshot");
        assert_eq!(supercharge_tool_runtime::Tool::id(&BrowserNavigateTool).as_str(), "browser_navigate");
        assert_eq!(supercharge_tool_runtime::Tool::id(&BrowserHandoffTool).as_str(), "browser_handoff");
    }

    #[test]
    fn rejects_invalid_browser_input() {
        assert!(valid_text("", "ref", 64).is_err());
        assert!(valid_text(&"x".repeat(MAX_TEXT_CHARS + 1), "text", MAX_TEXT_CHARS).is_err());
    }
}
