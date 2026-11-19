//! Agent tools for the built-in UI workflow.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::output::ToolOutput;
use crate::types::tool::{ToolKind, ToolNamespace};

use super::verify;

pub const UI_VERIFY: &str = "ui_verify";

/// Tools registered while the UI skill or design mode is active.
pub const UI_TOOL_NAMES: &[&str] = &[UI_VERIFY];

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct UiVerifyInput {
    /// Checks to run, in order: any of `typecheck`, `build`, `lint`. Default: typecheck then build.
    #[serde(default)]
    pub checks: Vec<String>,
    /// Folder of the frontend project, absolute or relative to the workspace. Default: detected from the workspace.
    #[serde(default)]
    pub project_dir: Option<String>,
    /// Seconds allowed for each command (default 600, max 1800).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Default)]
pub struct UiVerifyImpl;

impl crate::types::tool_metadata::ToolMetadata for UiVerifyImpl {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }
    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::SuperchargeBuild
    }
    fn description_template(&self) -> &str {
        "Run the frontend project's typecheck and build (and optionally lint) with its own package manager \
(bun, pnpm, yarn or npm, picked from the lockfile). Stops at the first failing check and returns each \
command, exit code and the end of its output as JSON. Call it after creating or changing a UI component, \
fix what it reports, and call it again until `passed` is true."
    }
}

impl supercharge_tool_runtime::Tool for UiVerifyImpl {
    type Args = UiVerifyInput;
    type Output = ToolOutput;

    fn id(&self) -> supercharge_tool_protocol::ToolId {
        supercharge_tool_protocol::ToolId::new(UI_VERIFY).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::supercharge_tool_runtime::ListToolsContext,
    ) -> supercharge_tool_types::ToolDescription {
        supercharge_tool_types::ToolDescription::new(
            UI_VERIFY,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> supercharge_tool_protocol::ToolCapabilities {
        supercharge_tool_protocol::ToolCapabilities {
            is_read_only: false,
            tool_scope: Some(supercharge_tool_protocol::ToolScope::Write),
            ..Default::default()
        }
    }

    async fn run(
        &self,
        ctx: supercharge_tool_runtime::ToolCallContext,
        input: UiVerifyInput,
    ) -> Result<ToolOutput, supercharge_tool_runtime::ToolError> {
        let resources = crate::types::tool_metadata::shared_resources(&ctx)?;
        let cwd = crate::types::tool_metadata::resolve_cwd(&ctx, &resources).await?;
        Ok(ToolOutput::Text(run_verify(&cwd, input).await.into()))
    }
}

/// Run `ui_verify` for `cwd` and render the JSON reply (errors included).
pub async fn run_verify(cwd: &std::path::Path, input: UiVerifyInput) -> String {
    let checks = match verify::parse_checks(&input.checks) {
        Ok(checks) => checks,
        Err(error) => return error_json(&error),
    };
    let timeout = input
        .timeout_secs
        .map(std::time::Duration::from_secs)
        .unwrap_or(verify::DEFAULT_TIMEOUT)
        .clamp(std::time::Duration::from_secs(1), verify::MAX_TIMEOUT);
    let dir = match input.project_dir.as_deref().map(str::trim) {
        Some(dir) if !dir.is_empty() => cwd.join(dir),
        _ => cwd.to_path_buf(),
    };
    match verify::verify_workspace(&dir, &checks, timeout).await {
        Ok(report) => serde_json::to_string_pretty(&report)
            .unwrap_or_else(|e| error_json(&format!("could not render the report: {e}"))),
        Err(error) => error_json(&error),
    }
}

fn error_json(message: &str) -> String {
    serde_json::json!({ "passed": false, "error": message }).to_string()
}

/// Register the UI tools on `bridge` when they are missing.
pub fn register_all(
    bridge: &crate::bridge::ToolBridge,
) -> Result<(), supercharge_tool_runtime::ToolError> {
    let toolset = bridge.toolset();
    if toolset.get_tool_metadata(UI_VERIFY).is_none() {
        bridge.register_mcp_tools(UI_VERIFY.to_owned(), UiVerifyImpl, None)?;
    }
    Ok(())
}

/// Remove the UI tools from `bridge`; returns how many were registered.
pub fn unregister_all(bridge: &crate::bridge::ToolBridge) -> usize {
    UI_TOOL_NAMES
        .iter()
        .filter(|name| bridge.unregister_tool_by_name(name))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_id_matches_constant() {
        use supercharge_tool_runtime::Tool;
        assert_eq!(UiVerifyImpl.id().to_string(), UI_VERIFY);
    }

    #[test]
    fn register_is_idempotent_and_unregister_removes_all() {
        let bridge = crate::bridge::ToolBridge::for_test();
        register_all(&bridge).expect("first register");
        register_all(&bridge).expect("second register is a no-op");
        for name in UI_TOOL_NAMES {
            assert!(bridge.toolset().get_tool_metadata(name).is_some(), "{name}");
        }
        assert_eq!(unregister_all(&bridge), UI_TOOL_NAMES.len());
        assert_eq!(unregister_all(&bridge), 0);
    }

    #[tokio::test]
    async fn bad_input_and_missing_project_return_json_errors() {
        let dir = tempfile::tempdir().unwrap();
        let bad: serde_json::Value = serde_json::from_str(
            &run_verify(
                dir.path(),
                UiVerifyInput {
                    checks: vec!["deploy".into()],
                    ..Default::default()
                },
            )
            .await,
        )
        .unwrap();
        assert_eq!(bad["passed"], false);
        assert!(bad["error"].as_str().unwrap().contains("unknown check"));

        let none: serde_json::Value =
            serde_json::from_str(&run_verify(dir.path(), UiVerifyInput::default()).await).unwrap();
        assert!(
            none["error"]
                .as_str()
                .unwrap()
                .contains("no frontend project")
        );
    }
}
