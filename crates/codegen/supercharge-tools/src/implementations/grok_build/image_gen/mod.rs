//! `image_gen` tool — generates images via an OpenAI-compatible
//! `/images/generations` endpoint (or a custom compatible host) and saves
//! them to the local filesystem so the model can reference them in code
//! (e.g. `<img src="images/hero.jpg">`).
//!
//! Architecture follows the same pattern as `web_search`:
//!
//! - [`ImageGenConfig`] is built from session credentials by the host and
//!   injected into the tool registry.
//! - When `Enabled`, an [`ImageGenClient`] is constructed once and injected
//!   into `Resources`. The tool reads it at runtime via `resources.require()`.
//! - When `Disabled`, the tool is not registered so the model never sees it.
//!
//! The generated image is written to `<session_folder>/images/<n>.jpg`
//! where `<n>` is a session-scoped counter (1, 2, 3, ... — 1 token each).
//! The tool returns the absolute path so the model can copy or move the
//! image into the project working directory when it needs a persistent asset.

use base64::Engine as _;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};

use crate::attribution::{SharedAttributionCallback, ToolConsumer};
use crate::types::SharedApiKeyProvider;

use crate::types::output::{MediaGenOutput, ToolOutput};
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::resources::SessionFolder;
use crate::types::tool::{ToolKind, ToolNamespace};

// Some hosts expand the prompt then generate, and the proxy buffers
// the whole image before sending any bytes — so the client may receive nothing
// for well over a minute. Keep these generous so a slow-but-progressing
// generation isn't cut off.
const IMAGE_GEN_TIMEOUT_SECS: u64 = 300;
const IMAGE_GEN_READ_TIMEOUT_SECS: u64 = 240;
const DEFAULT_IMAGE_DIR: &str = "images";

/// Wire format for `POST {base}/images/generations`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ImageProviderFormat {
    #[default]
    OpenaiImages,
    Pollinations,
    Custom,
}

impl ImageProviderFormat {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "pollinations" => Self::Pollinations,
            "custom" => Self::Custom,
            _ => Self::OpenaiImages,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiImages => "openai_images",
            Self::Pollinations => "pollinations",
            Self::Custom => "custom",
        }
    }
}

/// Map a tool `aspect_ratio` onto an OpenAI Images `size` string.
pub fn size_from_aspect_ratio(aspect: &str) -> Option<&'static str> {
    match aspect.trim() {
        "1:1" => Some("1024x1024"),
        "16:9" | "3:2" => Some("1792x1024"),
        "9:16" | "2:3" => Some("1024x1792"),
        "4:3" => Some("1024x768"),
        "3:4" => Some("768x1024"),
        _ => None,
    }
}

pub use supercharge_tools_api::slash_commands::{
    IMAGE_GEN_TOOL_NAME, IMAGINE_COMMAND_NAME, imagine_instruction, imagine_usage_message,
};

/// HTTP client for an OpenAI-compatible image API. Cloned per-request; shares `Arc` state.
#[derive(Clone)]
pub struct ImageGenClient {
    http: reqwest::Client,
    base_url: String,
    /// Imagine model slug used by `generate()`. Selected at construction
    /// from `ImageGenConfig::model_override` (falling back to
    /// [`XAI_IMAGINE_MODEL`]). `image_edit` uses its own model and is
    /// unaffected.
    model: String,
    edit_model: String,
    format: ImageProviderFormat,
    writer: super::storage::SessionFileWriter,
    api_key_provider: Option<SharedApiKeyProvider>,
    /// Optional 401-attribution hook. Hosts wire this so a 401 from the
    /// image API emits an `auth_401_attribution` event with
    /// `consumer == "ImageGen"` for unified auth-failure telemetry.
    attribution_callback: Option<SharedAttributionCallback>,
}

impl ImageGenClient {
    pub fn new(
        config: &ImageGenConfig,
        api_key_provider: Option<SharedApiKeyProvider>,
    ) -> Result<Self, supercharge_tool_runtime::ToolError> {
        let ImageGenConfig::Enabled {
            api_key,
            base_url,
            extra_headers,
            model_override,
            edit_model_override,
            format,
            ..
        } = config
        else {
            return Err(supercharge_tool_runtime::ToolError::invalid_arguments(
                "Cannot create ImageGenClient from disabled config",
            ));
        };
        let model = model_override
            .clone()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_default();
        let edit_model = edit_model_override
            .clone()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| model.clone());

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        // Always bake the static api_key as the default Authorization header.
        // The dynamic provider overrides per-request; this is the fallback.
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|e| {
                supercharge_tool_runtime::ToolError::invalid_arguments(format!(
                    "Invalid API key for header: {e}"
                ))
            })?,
        );

        extra_headers.into_iter().try_for_each(|(key, value)| {
            let header_name =
                reqwest::header::HeaderName::from_bytes(key.as_bytes()).map_err(|e| {
                    supercharge_tool_runtime::ToolError::invalid_arguments(format!(
                        "Invalid header name '{key}': {e}"
                    ))
                })?;
            let header_value = HeaderValue::from_str(value).map_err(|e| {
                supercharge_tool_runtime::ToolError::invalid_arguments(format!(
                    "Invalid header value for '{key}': {e}"
                ))
            })?;
            headers.insert(header_name, header_value);
            Ok::<(), supercharge_tool_runtime::ToolError>(())
        })?;

        let http = supercharge_extra_ca::with_extra_root_certificates(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(IMAGE_GEN_TIMEOUT_SECS))
                .read_timeout(std::time::Duration::from_secs(IMAGE_GEN_READ_TIMEOUT_SECS))
                .default_headers(headers),
        )
        .build()
        .map_err(|e| {
            supercharge_tool_runtime::ToolError::invalid_arguments(format!(
                "Failed to build HTTP client: {e}"
            ))
        })?;

        Ok(Self {
            http,
            base_url: base_url.clone(),
            model,
            edit_model,
            format: *format,
            writer: super::storage::SessionFileWriter::new(DEFAULT_IMAGE_DIR, "jpg"),
            api_key_provider,
            attribution_callback: None,
        })
    }

    /// Wire a 401-attribution callback into this client. Idempotent;
    /// safe to call before or after the first request. Builder-style
    /// so `new()` callers that don't care can ignore it.
    pub fn with_attribution_callback(
        mut self,
        callback: Option<SharedAttributionCallback>,
    ) -> Self {
        self.attribution_callback = callback;
        self
    }

    pub(crate) async fn current_bearer(&self) -> Option<String> {
        crate::types::api_key_provider::resolve_bearer(self.api_key_provider.as_ref()).await
    }

    pub(crate) fn record_401_attribution(&self, consumer: ToolConsumer, sent_bearer: Option<&str>) {
        crate::attribution::emit_401(self.attribution_callback.as_ref(), consumer, sent_bearer);
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub(crate) fn writer(&self) -> &super::storage::SessionFileWriter {
        &self.writer
    }

    pub(crate) fn edit_model(&self) -> &str {
        &self.edit_model
    }

    pub async fn generate(
        &self,
        prompt: &str,
        aspect_ratio: &str,
    ) -> Result<Vec<u8>, supercharge_tool_runtime::ToolError> {
        if self.model.trim().is_empty() {
            return Err(supercharge_tool_runtime::ToolError::invalid_arguments(
                "No image model configured. Run /isetup or /imodels to pick a model.",
            ));
        }

        let url = format!("{}/images/generations", self.base_url.trim_end_matches('/'));

        let mut payload = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            "n": 1,
            "response_format": "b64_json",
        });
        if let Some(size) = size_from_aspect_ratio(aspect_ratio) {
            payload["size"] = serde_json::Value::String(size.to_owned());
        }
        if self.format == ImageProviderFormat::Custom && !aspect_ratio.trim().is_empty() {
            payload["aspect_ratio"] = serde_json::Value::String(aspect_ratio.to_owned());
        }

        // Capture the bearer once so the request and the 401-attribution
        // emit see the same value (even if the provider rotates between
        // the send and the response handling).
        let sent_bearer = self.current_bearer().await;
        let mut req = self.http.post(&url).json(&payload);
        if let Some(ref key) = sent_bearer {
            req = req.header(AUTHORIZATION, format!("Bearer {key}"));
        }

        let response = req.send().await.map_err(|e| {
            supercharge_tool_runtime::ToolError::invalid_arguments(format!(
                "Image generation API request failed: {e}"
            ))
        })?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            self.record_401_attribution(ToolConsumer::ImageGen, sent_bearer.as_deref());
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let truncated: String = body.chars().take(200).collect();
            tracing::warn!(http_status = %status, "Image generation API error: {truncated}");
            return Err(supercharge_tool_runtime::ToolError::new(
                supercharge_tool_runtime::ToolErrorKind::Custom,
                format!("Image generation failed with HTTP {status}: {truncated}"),
            )
            .with_details(serde_json::json!({"code": "http_failure", "status": status.as_u16()})));
        }

        let body = response.text().await.map_err(|e| {
            supercharge_tool_runtime::ToolError::invalid_arguments(format!(
                "Failed to read image generation response body: {e}"
            ))
        })?;

        let resp_json: ImageGenResponse = serde_json::from_str(&body).map_err(|e| {
            let preview: String = body.chars().take(500).collect();
            tracing::warn!("Image generation API returned unparseable body: {preview}");
            supercharge_tool_runtime::ToolError::invalid_arguments(format!(
                "Failed to parse image generation response: {e} — body preview: {preview}"
            ))
        })?;

        if let Some(b64_data) = resp_json.b64_data().filter(|s| !s.is_empty()) {
            return base64::engine::general_purpose::STANDARD
                .decode(b64_data)
                .map_err(|e| {
                    supercharge_tool_runtime::ToolError::invalid_arguments(format!(
                        "Failed to decode base64 image data: {e}"
                    ))
                });
        }

        if let Some(image_url) = resp_json.url().filter(|s| !s.is_empty()) {
            let response = self.http.get(image_url).send().await.map_err(|e| {
                supercharge_tool_runtime::ToolError::invalid_arguments(format!(
                    "Failed to download generated image: {e}"
                ))
            })?;
            if !response.status().is_success() {
                return Err(supercharge_tool_runtime::ToolError::new(
                    supercharge_tool_runtime::ToolErrorKind::Custom,
                    format!(
                        "Image download failed with HTTP {}",
                        response.status()
                    ),
                ));
            }
            return response.bytes().await.map(|b| b.to_vec()).map_err(|e| {
                supercharge_tool_runtime::ToolError::invalid_arguments(format!(
                    "Failed to read generated image bytes: {e}"
                ))
            });
        }

        Err(supercharge_tool_runtime::ToolError::invalid_arguments(
            "Image generation returned no image data.",
        ))
    }
}

/// `Enabled` means credentials are present; each tool has its own gate.
#[derive(Debug, Clone, Default)]
pub enum ImageGenConfig {
    #[default]
    Disabled,
    Enabled {
        api_key: String,
        base_url: String,
        extra_headers: indexmap::IndexMap<String, String>,
        image_gen_enabled: bool,
        image_edit_enabled: bool,
        /// Image model slug. Empty until `/isetup` or `/imodels` sets one.
        model_override: Option<String>,
        edit_model_override: Option<String>,
        format: ImageProviderFormat,
    },
}

/// Optional session-id header some gateways still accept.
pub const SESSION_ID_HEADER: &str = "x-supercharge-session-id";

impl ImageGenConfig {
    /// Credentials present — required to construct any of the clients.
    pub fn has_credentials(&self) -> bool {
        matches!(self, Self::Enabled { .. })
    }

    /// Stamp [`SESSION_ID_HEADER`] onto `extra_headers`. A caller-provided
    /// value is never overwritten. No-op when `Disabled`.
    pub fn stamp_session_id_header(&mut self, session_id: &str) {
        if let Self::Enabled { extra_headers, .. } = self {
            extra_headers
                .entry(SESSION_ID_HEADER.to_string())
                .or_insert_with(|| session_id.to_string());
        }
    }

    pub fn image_gen_enabled(&self) -> bool {
        matches!(
            self,
            Self::Enabled {
                image_gen_enabled: true,
                ..
            }
        )
    }

    pub fn image_edit_enabled(&self) -> bool {
        matches!(
            self,
            Self::Enabled {
                image_edit_enabled: true,
                ..
            }
        )
    }

    /// The configured `image_gen` model, if any.
    pub fn model_override(&self) -> Option<&str> {
        match self {
            Self::Enabled { model_override, .. } => {
                model_override.as_deref().filter(|m| !m.trim().is_empty())
            }
            Self::Disabled => None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ImageGenInput {
    #[schemars(description = "Text description of the image to generate.")]
    pub prompt: String,

    #[serde(default = "default_aspect_ratio")]
    #[schemars(
        description = "Aspect ratio of the generated image, decide it based on the user's request. Defaults to 'auto'. 1:1 for square (icons, profiles), 16:9 for wide (landscapes, cinematic), 9:16 for tall (phone wallpapers, stories), 3:2 for horizontal photos, 2:3 for vertical (portraits, posters)."
    )]
    pub aspect_ratio: String,
}

fn default_aspect_ratio() -> String {
    "auto".to_owned()
}

#[derive(Debug, serde::Deserialize)]
pub struct ImageGenResponse {
    #[serde(default)]
    data: Vec<ImageGenData>,
}

impl ImageGenResponse {
    pub fn b64_data(&self) -> Option<&str> {
        self.data.first().and_then(|d| d.b64_json.as_deref())
    }

    pub fn url(&self) -> Option<&str> {
        self.data.first().and_then(|d| d.url.as_deref())
    }
}

#[derive(Debug, serde::Deserialize)]
struct ImageGenData {
    b64_json: Option<String>,
    url: Option<String>,
}

#[derive(Debug, Default)]
pub struct ImageGenTool;

impl crate::types::tool_metadata::ToolMetadata for ImageGenTool {
    fn kind(&self) -> ToolKind {
        ToolKind::ImageGen
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Generate a new image from a text description; returns the saved image's absolute path. When telling the user where it was saved, refer to it by its short session-relative path (e.g. `images/1.jpg`) rather than the absolute path, so it renders as a clickable link that opens the image. To produce multiple images, emit multiple tool calls with distinct prompts."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl supercharge_tool_runtime::Tool for ImageGenTool {
    type Args = ImageGenInput;
    type Output = ToolOutput;

    fn id(&self) -> supercharge_tool_protocol::ToolId {
        supercharge_tool_protocol::ToolId::new("image_gen").expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::supercharge_tool_runtime::ListToolsContext,
    ) -> supercharge_tool_types::ToolDescription {
        supercharge_tool_types::ToolDescription::new(
            "image_gen",
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

    #[tracing::instrument(
        name = "tool.image_gen",
        skip_all,
        fields(prompt_len = input.prompt.len(), aspect_ratio = %input.aspect_ratio)
    )]
    async fn run(
        &self,
        ctx: supercharge_tool_runtime::ToolCallContext,
        input: ImageGenInput,
    ) -> Result<ToolOutput, supercharge_tool_runtime::ToolError> {
        use crate::types::tool_metadata::shared_resources;
        let resources = shared_resources(&ctx)?;

        let client = {
            let res = resources.lock().await;
            res.require::<ImageGenClient>()?.clone()
        };

        let image_bytes = client.generate(&input.prompt, &input.aspect_ratio).await?;

        let session_folder = {
            let res = resources.lock().await;
            res.require::<SessionFolder>()?.0.clone()
        };

        let absolute_path = client
            .writer
            .save(&session_folder, &image_bytes, None)
            .await
            .map_err(|e| supercharge_tool_runtime::ToolError::invalid_arguments(e.to_string()))?;

        tracing::info!(
            path = %absolute_path.display(),
            bytes = image_bytes.len(),
            "image saved to disk"
        );

        Ok(ToolOutput::ImageGen(MediaGenOutput::new(absolute_path)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::tool_metadata::test_ctx_with_call_id;

    #[test]
    fn tool_name_and_description() {
        let tool = ImageGenTool;
        assert_eq!(supercharge_tool_runtime::Tool::id(&tool).as_str(), "image_gen");
        assert!(
            crate::types::tool_metadata::ToolMetadata::description_template(&tool)
                .contains("Generate a new image from a text description")
        );
    }

    #[test]
    fn default_aspect_ratio_is_auto() {
        let input: ImageGenInput = serde_json::from_str(r#"{"prompt": "test"}"#).unwrap();
        assert_eq!(input.aspect_ratio, "auto");
    }

    #[test]
    fn per_tool_gates_are_independent() {
        let cfg = ImageGenConfig::Enabled {
            api_key: "k".into(),
            base_url: "https://api.x.ai/v1".into(),
            extra_headers: indexmap::IndexMap::new(),
            image_gen_enabled: false,
            image_edit_enabled: true,
            model_override: Some("flux".into()),
            edit_model_override: None,
            format: ImageProviderFormat::OpenaiImages,
        };
        assert!(cfg.has_credentials());
        assert!(!cfg.image_gen_enabled());
        assert!(cfg.image_edit_enabled());
        assert_eq!(cfg.model_override(), Some("flux"));

        assert!(!ImageGenConfig::Disabled.has_credentials());
    }

    #[test]
    fn stamp_session_id_header_sets_and_preserves() {
        let mk = |headers: indexmap::IndexMap<String, String>| ImageGenConfig::Enabled {
            api_key: "k".into(),
            base_url: "https://api.x.ai/v1".into(),
            extra_headers: headers,
            image_gen_enabled: true,
            image_edit_enabled: true,
            model_override: None,
            edit_model_override: None,
            format: ImageProviderFormat::OpenaiImages,
        };
        let hdrs = |cfg: &ImageGenConfig| match cfg {
            ImageGenConfig::Enabled { extra_headers, .. } => extra_headers.clone(),
            _ => unreachable!(),
        };

        let mut cfg = mk(indexmap::IndexMap::new());
        cfg.stamp_session_id_header("sess-123");
        assert_eq!(
            hdrs(&cfg).get(SESSION_ID_HEADER).map(String::as_str),
            Some("sess-123")
        );

        let mut preset = indexmap::IndexMap::new();
        preset.insert(SESSION_ID_HEADER.to_string(), "caller-set".to_string());
        let mut cfg = mk(preset);
        cfg.stamp_session_id_header("sess-123");
        assert_eq!(
            hdrs(&cfg).get(SESSION_ID_HEADER).map(String::as_str),
            Some("caller-set")
        );

        let mut disabled = ImageGenConfig::Disabled;
        disabled.stamp_session_id_header("sess-123");
        assert!(!disabled.has_credentials());
    }

    #[test]
    fn client_selects_model_from_override() {
        let mk = |model_override: Option<&str>| ImageGenConfig::Enabled {
            api_key: "k".into(),
            base_url: "https://api.x.ai/v1".into(),
            extra_headers: indexmap::IndexMap::new(),
            image_gen_enabled: true,
            image_edit_enabled: true,
            model_override: model_override.map(String::from),
            edit_model_override: None,
            format: ImageProviderFormat::OpenaiImages,
        };
        assert_eq!(ImageGenClient::new(&mk(None), None).unwrap().model, "");
        assert_eq!(ImageGenClient::new(&mk(Some("")), None).unwrap().model, "");
        assert_eq!(
            ImageGenClient::new(&mk(Some("flux")), None).unwrap().model,
            "flux"
        );
    }

    #[test]
    fn client_selects_edit_model_from_override() {
        let mk = |edit_model_override: Option<&str>| ImageGenConfig::Enabled {
            api_key: "k".into(),
            base_url: "https://api.x.ai/v1".into(),
            extra_headers: indexmap::IndexMap::new(),
            image_gen_enabled: true,
            image_edit_enabled: true,
            model_override: Some("flux".into()),
            edit_model_override: edit_model_override.map(String::from),
            format: ImageProviderFormat::OpenaiImages,
        };
        assert_eq!(
            ImageGenClient::new(&mk(None), None).unwrap().edit_model(),
            "flux"
        );
        assert_eq!(
            ImageGenClient::new(&mk(Some("  ")), None)
                .unwrap()
                .edit_model(),
            "flux"
        );
        let client = ImageGenClient::new(&mk(Some("edit-v2")), None).unwrap();
        assert_eq!(client.edit_model(), "edit-v2");
        assert_eq!(client.model, "flux");
    }

    #[tokio::test]
    async fn errors_when_client_missing() {
        let tool = ImageGenTool;
        let resources = crate::types::resources::Resources::new();
        let result = supercharge_tool_runtime::Tool::run(
            &tool,
            test_ctx_with_call_id(resources.into_shared(), "test-call"),
            ImageGenInput {
                prompt: "a test image".into(),
                aspect_ratio: "auto".into(),
            },
        )
        .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("missing required resource"),
            "Expected MissingResource error, got: {err_msg}"
        );
    }

    #[test]
    fn size_from_aspect_ratio_maps_common_ratios() {
        assert_eq!(size_from_aspect_ratio("1:1"), Some("1024x1024"));
        assert_eq!(size_from_aspect_ratio("16:9"), Some("1792x1024"));
        assert_eq!(size_from_aspect_ratio("9:16"), Some("1024x1792"));
        assert_eq!(size_from_aspect_ratio("auto"), None);
    }

    #[test]
    fn image_provider_format_parses_custom_ids() {
        assert_eq!(
            ImageProviderFormat::parse("pollinations"),
            ImageProviderFormat::Pollinations
        );
        assert_eq!(
            ImageProviderFormat::parse("custom"),
            ImageProviderFormat::Custom
        );
        assert_eq!(
            ImageProviderFormat::parse("openai_images"),
            ImageProviderFormat::OpenaiImages
        );
    }
}
