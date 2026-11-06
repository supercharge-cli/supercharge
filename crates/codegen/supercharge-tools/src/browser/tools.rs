//! Local headless browser agent tools, gated on the optional installed binary.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::install::is_installed;
use super::runtime::{BrowserRuntime, INSTALL_HINT};
use crate::registry::types::ToolConfig;
use crate::types::output::{DesktopObservation, DynamicOutput, ImageContent, ToolOutput};
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};
use crate::types::tool_io::ToolInput;

/// Unqualified wire ids. Distinct from E2B `browser_*` tools.
pub const LOCAL_BROWSER_TOOL_IDS: &[&str] = &[
    "local_browser_open",
    "local_browser_read",
    "local_browser_click",
    "local_browser_type",
    "local_browser_screenshot",
    "local_browser_evaluate",
];

const MAX_TEXT_CHARS: usize = 16_000;

/// Whether browser tools should be offered to the agent.
pub fn browser_tools_available() -> bool {
    is_installed()
}

/// Tool configs for the local browser pack (always the six callable tools).
pub fn local_browser_tool_configs() -> Vec<ToolConfig> {
    vec![
        (&LocalBrowserOpenTool).into(),
        (&LocalBrowserReadTool).into(),
        (&LocalBrowserClickTool).into(),
        (&LocalBrowserTypeTool).into(),
        (&LocalBrowserScreenshotTool).into(),
        (&LocalBrowserEvaluateTool).into(),
    ]
}

/// Same as [`local_browser_tool_configs`] when the binary is present, otherwise empty.
pub fn maybe_local_browser_tool_configs() -> Vec<ToolConfig> {
    if is_installed() {
        local_browser_tool_configs()
    } else {
        Vec::new()
    }
}

pub fn browser_unavailable_error() -> supercharge_tool_runtime::ToolError {
    supercharge_tool_runtime::ToolError::custom("browser_unavailable", INSTALL_HINT)
}

fn require_installed() -> Result<(), supercharge_tool_runtime::ToolError> {
    if is_installed() {
        Ok(())
    } else {
        Err(browser_unavailable_error())
    }
}

fn map_cdp(err: super::cdp::CdpError) -> supercharge_tool_runtime::ToolError {
    let detail = err.to_string();
    if detail.contains("not installed") {
        return browser_unavailable_error();
    }
    supercharge_tool_runtime::ToolError::custom("browser_cdp", detail)
}

fn valid_text(
    value: &str,
    label: &str,
    max: usize,
) -> Result<(), supercharge_tool_runtime::ToolError> {
    if value.trim().is_empty() || value.len() > max || value.contains('\0') {
        return Err(supercharge_tool_runtime::ToolError::invalid_arguments(
            format!("{label} is empty, invalid, or too long"),
        ));
    }
    Ok(())
}

/// Convenience: open URL, return visible text (for tests and scripting).
pub async fn open_and_read_text(url: &str) -> Result<String, super::cdp::CdpError> {
    let runtime = BrowserRuntime::global();
    let mut session = runtime.session(url).await?;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    session.page_text().await
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LocalBrowserOpenInput {
    /// HTTP, HTTPS, or workspace `file://` URL to open or navigate to.
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct LocalBrowserReadInput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LocalBrowserClickInput {
    /// CSS selector of the element to click.
    pub selector: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LocalBrowserTypeInput {
    /// CSS selector of the input or textarea.
    pub selector: String,
    /// Text to assign as the element's value.
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct LocalBrowserScreenshotInput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LocalBrowserEvaluateInput {
    /// JavaScript expression to evaluate in the page.
    pub expression: String,
}

macro_rules! into_dynamic_input {
    ($ty:ty) => {
        impl From<$ty> for ToolInput {
            fn from(value: $ty) -> Self {
                ToolInput::Dynamic(serde_json::to_value(value).unwrap_or(serde_json::Value::Null))
            }
        }
    };
}

into_dynamic_input!(LocalBrowserOpenInput);
into_dynamic_input!(LocalBrowserReadInput);
into_dynamic_input!(LocalBrowserClickInput);
into_dynamic_input!(LocalBrowserTypeInput);
into_dynamic_input!(LocalBrowserScreenshotInput);
into_dynamic_input!(LocalBrowserEvaluateInput);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalBrowserToolOutput {
    pub result: serde_json::Value,
}

impl From<LocalBrowserToolOutput> for ToolOutput {
    fn from(mut value: LocalBrowserToolOutput) -> Self {
        if let Some(data) = value
            .result
            .as_object_mut()
            .and_then(|result| result.remove("png"))
            .and_then(|png| png.as_str().map(str::to_owned))
        {
            return ToolOutput::DesktopObservation(DesktopObservation {
                image: ImageContent {
                    data,
                    mime_type: "image/png".into(),
                    annotations: None,
                    uri: None,
                    meta: None,
                },
                metadata: value.result,
            });
        }
        ToolOutput::Dynamic(DynamicOutput::from(value.result))
    }
}

impl supercharge_tool_runtime::ToolOutput for LocalBrowserToolOutput {}

fn ok_json(value: serde_json::Value) -> LocalBrowserToolOutput {
    LocalBrowserToolOutput { result: value }
}

macro_rules! local_browser_tool {
    (
        $name:ident,
        $input:ty,
        $id:literal,
        $description:literal,
        $readonly:expr,
        |$arg:ident| $body:block
    ) => {
        #[derive(Debug, Default)]
        pub struct $name;

        impl crate::types::tool_metadata::ToolMetadata for $name {
            fn kind(&self) -> ToolKind {
                ToolKind::Other
            }
            fn tool_namespace(&self) -> ToolNamespace {
                ToolNamespace::SuperchargeBuild
            }
            fn description_template(&self) -> &str {
                $description
            }
            fn requires_expr(&self) -> Expr<ToolRequirement> {
                Expr::True
            }
            fn is_read_only(&self) -> bool {
                $readonly
            }
        }

        impl supercharge_tool_runtime::Tool for $name {
            type Args = $input;
            type Output = LocalBrowserToolOutput;
            fn id(&self) -> supercharge_tool_protocol::ToolId {
                supercharge_tool_protocol::ToolId::new($id).expect("valid local browser tool id")
            }
            fn description(
                &self,
                _ctx: &supercharge_tool_runtime::ListToolsContext,
            ) -> supercharge_tool_types::ToolDescription {
                supercharge_tool_types::ToolDescription::new(
                    $id,
                    crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
                )
            }
            fn capabilities(&self) -> supercharge_tool_protocol::ToolCapabilities {
                supercharge_tool_protocol::ToolCapabilities {
                    is_read_only: $readonly,
                    tool_scope: Some(if $readonly {
                        supercharge_tool_protocol::ToolScope::Read
                    } else {
                        supercharge_tool_protocol::ToolScope::Write
                    }),
                    ..Default::default()
                }
            }
            async fn run(
                &self,
                _ctx: supercharge_tool_runtime::ToolCallContext,
                $arg: $input,
            ) -> Result<LocalBrowserToolOutput, supercharge_tool_runtime::ToolError> {
                require_installed()?;
                $body
            }
        }
    };
}

local_browser_tool!(
    LocalBrowserOpenTool,
    LocalBrowserOpenInput,
    "local_browser_open",
    "Open or navigate the optional local headless browser to a URL. Install with `supercharge browser install`. Use local_browser_read after navigation.",
    false,
    |input| {
        valid_text(&input.url, "url", 2_048)?;
        BrowserRuntime::global()
            .open_url(&input.url)
            .await
            .map_err(map_cdp)?;
        Ok(ok_json(serde_json::json!({ "ok": true, "url": input.url })))
    }
);

local_browser_tool!(
    LocalBrowserReadTool,
    LocalBrowserReadInput,
    "local_browser_read",
    "Read visible text from the current local browser page. Call local_browser_open first.",
    true,
    |_input| {
        let text = BrowserRuntime::global()
            .page_text()
            .await
            .map_err(map_cdp)?;
        Ok(ok_json(serde_json::json!({ "ok": true, "text": text })))
    }
);

local_browser_tool!(
    LocalBrowserClickTool,
    LocalBrowserClickInput,
    "local_browser_click",
    "Click the first element matching a CSS selector in the current local browser page.",
    false,
    |input| {
        valid_text(&input.selector, "selector", 2_048)?;
        BrowserRuntime::global()
            .click_selector(&input.selector)
            .await
            .map_err(map_cdp)?;
        Ok(ok_json(serde_json::json!({
            "ok": true,
            "selector": input.selector
        })))
    }
);

local_browser_tool!(
    LocalBrowserTypeTool,
    LocalBrowserTypeInput,
    "local_browser_type",
    "Type text into the first element matching a CSS selector in the current local browser page.",
    false,
    |input| {
        valid_text(&input.selector, "selector", 2_048)?;
        valid_text(&input.text, "text", MAX_TEXT_CHARS)?;
        BrowserRuntime::global()
            .type_selector(&input.selector, &input.text)
            .await
            .map_err(map_cdp)?;
        Ok(ok_json(serde_json::json!({
            "ok": true,
            "selector": input.selector
        })))
    }
);

local_browser_tool!(
    LocalBrowserScreenshotTool,
    LocalBrowserScreenshotInput,
    "local_browser_screenshot",
    "Capture a PNG screenshot of the current local browser page.",
    true,
    |_input| {
        let png = BrowserRuntime::global()
            .screenshot_png()
            .await
            .map_err(map_cdp)?;
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(png);
        Ok(ok_json(serde_json::json!({ "ok": true, "png": data })))
    }
);

local_browser_tool!(
    LocalBrowserEvaluateTool,
    LocalBrowserEvaluateInput,
    "local_browser_evaluate",
    "Evaluate a JavaScript expression in the current local browser page and return the value.",
    false,
    |input| {
        valid_text(&input.expression, "expression", MAX_TEXT_CHARS)?;
        let value = BrowserRuntime::global()
            .evaluate(&input.expression)
            .await
            .map_err(map_cdp)?;
        Ok(ok_json(serde_json::json!({ "ok": true, "value": value })))
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::types::ToolRegistryBuilder;
    use crate::types::tool_metadata::test_ctx_with_call_id;

    #[test]
    fn wire_ids_are_stable_and_do_not_collide_with_cloud_browser_tools() {
        use supercharge_tool_runtime::Tool;
        let ids = [
            LocalBrowserOpenTool.id().to_string(),
            LocalBrowserReadTool.id().to_string(),
            LocalBrowserClickTool.id().to_string(),
            LocalBrowserTypeTool.id().to_string(),
            LocalBrowserScreenshotTool.id().to_string(),
            LocalBrowserEvaluateTool.id().to_string(),
        ];
        assert_eq!(
            ids.iter().map(String::as_str).collect::<Vec<_>>(),
            LOCAL_BROWSER_TOOL_IDS
        );
        for id in ids {
            assert!(
                !id.starts_with("browser_"),
                "local tool `{id}` collides with cloud browser_* ids"
            );
        }
    }

    #[test]
    fn input_schemas_include_required_fields() {
        let open = serde_json::to_value(schemars::schema_for!(LocalBrowserOpenInput)).unwrap();
        let open_text = open.to_string();
        assert!(open_text.contains("url"), "{open_text}");

        let click = serde_json::to_value(schemars::schema_for!(LocalBrowserClickInput)).unwrap();
        assert!(click.to_string().contains("selector"));

        let typed = serde_json::to_value(schemars::schema_for!(LocalBrowserTypeInput)).unwrap();
        let typed_text = typed.to_string();
        assert!(typed_text.contains("selector"));
        assert!(typed_text.contains("text"));

        let eval = serde_json::to_value(schemars::schema_for!(LocalBrowserEvaluateInput)).unwrap();
        assert!(eval.to_string().contains("expression"));
    }

    #[test]
    fn global_registry_always_registers_local_and_cloud_browser_ids() {
        let builder = ToolRegistryBuilder::new();
        for id in LOCAL_BROWSER_TOOL_IDS {
            let fq = format!("SuperchargeBuild:{id}");
            assert!(builder.has_tool_id(&fq), "missing {fq}");
        }
        assert!(builder.has_tool_id("SuperchargeBuild:browser_navigate"));
        assert!(builder.has_tool_id("SuperchargeBuild:browser_screenshot"));
        assert!(builder.has_tool_id("SuperchargeBuild:browser_handoff"));
    }

    #[test]
    fn maybe_configs_follow_install_flag() {
        assert_eq!(local_browser_tool_configs().len(), 6);
        let expected = if is_installed() { 6 } else { 0 };
        assert_eq!(maybe_local_browser_tool_configs().len(), expected);
    }

    #[test]
    fn unavailable_error_is_an_install_hint() {
        let err = browser_unavailable_error();
        let text = err.to_string();
        assert!(
            text.contains("supercharge browser install"),
            "expected install hint, got {text}"
        );
        assert!(!text.to_lowercase().contains("panic"));
    }

    #[tokio::test]
    async fn calling_open_without_binary_returns_install_hint() {
        if is_installed() {
            // Host already has the optional binary; the error constructor is still asserted above.
            return;
        }
        let resources = crate::types::resources::Resources::new();
        let err = supercharge_tool_runtime::Tool::run(
            &LocalBrowserOpenTool,
            test_ctx_with_call_id(resources.into_shared(), "local-browser-missing"),
            LocalBrowserOpenInput {
                url: "https://example.test".into(),
            },
        )
        .await
        .expect_err("missing binary must not panic");
        let text = err.to_string();
        assert!(
            text.contains("supercharge browser install"),
            "expected install hint, got {text}"
        );
    }

    #[test]
    fn png_results_become_desktop_observations() {
        let shot = LocalBrowserToolOutput {
            result: serde_json::json!({"ok": true, "png": "aGVsbG8=", "url": "https://example.test"}),
        };
        match ToolOutput::from(shot) {
            ToolOutput::DesktopObservation(observation) => {
                assert_eq!(observation.image.data, "aGVsbG8=");
                assert_eq!(observation.image.mime_type, "image/png");
                assert!(observation.metadata.get("png").is_none());
            }
            other => panic!("expected desktop observation, got {other:?}"),
        }
    }
}
