//! `web_search` tool — new architecture (`Tool` trait).
//!
//! Reads `WebSearchRouter` from Resources. Exa is preferred when a key is set,
//! Linkup is the fallback, and the Responses API client is used when neither key is set.

use crate::implementations::web_search::router::{SearchMode, WebSearchRouter};
use crate::types::output::WebSearchOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

// ───────────────────────────────────────────────────────────────────────────
// Input
// ───────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct WebSearchInput {
    #[schemars(description = "The search query to perform.")]
    pub query: String,
    #[schemars(description = "Optional list of domains to restrict search to.")]
    pub allowed_domains: Option<Vec<String>>,
    /// `auto` (default) or `lite` for a cheaper, lower-latency lookup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Search mode: \"auto\" (default) or \"lite\".")]
    pub mode: Option<String>,
    /// When set, fetch trimmed text for these URLs instead of running a query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional URLs whose page text should be fetched.")]
    pub urls: Option<Vec<String>>,
}

// ───────────────────────────────────────────────────────────────────────────
// Tool implementation
// ───────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct WebSearchTool;

impl crate::types::tool_metadata::ToolMetadata for WebSearchTool {
    fn kind(&self) -> ToolKind {
        ToolKind::WebSearch
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::SuperchargeBuild
    }

    fn description_template(&self) -> &str {
        "Search the web for up-to-date information, tailored for coding and software development tasks."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl supercharge_tool_runtime::Tool for WebSearchTool {
    type Args = WebSearchInput;
    type Output = WebSearchOutput;

    fn id(&self) -> supercharge_tool_protocol::ToolId {
        supercharge_tool_protocol::ToolId::new("web_search").expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::supercharge_tool_runtime::ListToolsContext,
    ) -> supercharge_tool_types::ToolDescription {
        supercharge_tool_types::ToolDescription::new(
            "web_search",
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> supercharge_tool_protocol::ToolCapabilities {
        supercharge_tool_protocol::ToolCapabilities {
            is_read_only: true,
            tool_scope: Some(supercharge_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    #[tracing::instrument(name = "tool.web_search", skip_all)]
    async fn run(
        &self,
        ctx: supercharge_tool_runtime::ToolCallContext,
        input: WebSearchInput,
    ) -> Result<WebSearchOutput, supercharge_tool_runtime::ToolError> {
        use crate::types::tool_metadata::shared_resources;
        let resources = shared_resources(&ctx)?;

        let client;
        {
            let res = resources.lock().await;
            client = res.require::<WebSearchRouter>()?.clone();
        }

        let mode = SearchMode::parse(input.mode.as_deref()).map_err(|msg| {
            supercharge_tool_runtime::ToolError::execution(
                supercharge_tool_protocol::ToolId::new("web_search").expect("valid"),
                msg,
            )
        })?;
        let query_span = tracing::info_span!("web_search.query");
        let (content, citations, pre_formatted) = client
            .search(
                &input.query,
                input.allowed_domains.clone(),
                mode,
                input.urls.clone(),
            )
            .await?;
        drop(query_span);

        Ok(WebSearchOutput {
            query: input.query.clone(),
            content,
            citations,
            allowed_domains: input.allowed_domains.clone(),
            pre_formatted,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::Resources;
    use crate::types::tool_metadata::test_ctx_with_call_id;

    #[test]
    fn tool_name_and_description() {
        let tool = WebSearchTool;
        assert_eq!(
            supercharge_tool_runtime::Tool::id(&tool).as_str(),
            "web_search"
        );
    }

    #[tokio::test]
    async fn errors_when_client_not_in_resources() {
        let resources = Resources::new();
        let tool = WebSearchTool;
        let result = supercharge_tool_runtime::Tool::run(
            &tool,
            test_ctx_with_call_id(resources.into_shared(), "test-call"),
            WebSearchInput {
                query: "test".into(),
                allowed_domains: None,
                mode: None,
                urls: None,
            },
        )
        .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("missing required resource"),
            "Expected 'missing required resource' error, got: {err_msg}"
        );
    }
}
