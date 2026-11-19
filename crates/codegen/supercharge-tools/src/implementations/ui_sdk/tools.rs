//! Agent tools for the built-in UI workflow.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::output::ToolOutput;
use crate::types::tool::{ToolKind, ToolNamespace};

use super::verify;

pub const UI_VERIFY: &str = "ui_verify";

/// Tools registered while the UI skill or design mode is active.
pub const UI_TOOL_NAMES: &[&str] = &[UI_VERIFY, UI_BROWSER];

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

pub const UI_BROWSER: &str = "ui_browser";

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct UiBrowserInput {
    /// One of `open`, `snapshot`, `click`, `type`, `press`, `close`.
    pub action: String,
    /// For `open`: the page to load, e.g. `http://localhost:5173/settings`.
    #[serde(default)]
    pub url: Option<String>,
    /// For `click`, `type` and `press`: an element ref such as `e12` from the latest snapshot.
    #[serde(default, rename = "ref", alias = "element")]
    pub element: Option<String>,
    /// For `type`: the text to enter.
    #[serde(default)]
    pub text: Option<String>,
    /// For `type`: clear the field first.
    #[serde(default)]
    pub clear: bool,
    /// For `press`: a key such as `Enter`, `Escape`, `ArrowDown`, `Shift+Tab` or `Control+a`.
    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Debug, Default)]
pub struct UiBrowserImpl;

impl crate::types::tool_metadata::ToolMetadata for UiBrowserImpl {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }
    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::SuperchargeBuild
    }
    fn description_template(&self) -> &str {
        "Check a running UI in a headless Chrome or Chromium without screenshots. `open` loads a URL \
(start the dev server in the background first) and returns the page's accessibility tree: roles, names, \
states such as expanded or checked, and refs like e12. `click`, `type` (with `text`, optional `clear`) and \
`press` (with `key`, optional `ref` to focus first) use real mouse and keyboard events on a ref, then return \
a fresh snapshot. `snapshot` re-reads the page; `close` stops the browser. Page errors are listed under \
the tree. Use it to confirm every control has a name, keyboard paths work, and states change as expected."
    }
}

impl supercharge_tool_runtime::Tool for UiBrowserImpl {
    type Args = UiBrowserInput;
    type Output = ToolOutput;

    fn id(&self) -> supercharge_tool_protocol::ToolId {
        supercharge_tool_protocol::ToolId::new(UI_BROWSER).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::supercharge_tool_runtime::ListToolsContext,
    ) -> supercharge_tool_types::ToolDescription {
        supercharge_tool_types::ToolDescription::new(
            UI_BROWSER,
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
        _ctx: supercharge_tool_runtime::ToolCallContext,
        input: UiBrowserInput,
    ) -> Result<ToolOutput, supercharge_tool_runtime::ToolError> {
        Ok(ToolOutput::Text(run_browser(input).await.into()))
    }
}

type SharedBrowser = tokio::sync::Mutex<Option<super::browser::UiBrowser>>;

fn shared_browser() -> &'static SharedBrowser {
    static BROWSER: std::sync::OnceLock<SharedBrowser> = std::sync::OnceLock::new();
    BROWSER.get_or_init(Default::default)
}

/// A browser left idle this long is closed, so it never lingers after the work is done.
const BROWSER_IDLE_LIMIT: std::time::Duration = std::time::Duration::from_secs(600);

fn idle_clock() -> &'static (std::time::Instant, std::sync::atomic::AtomicU64) {
    static CLOCK: std::sync::OnceLock<(std::time::Instant, std::sync::atomic::AtomicU64)> =
        std::sync::OnceLock::new();
    CLOCK.get_or_init(|| {
        (
            std::time::Instant::now(),
            std::sync::atomic::AtomicU64::new(0),
        )
    })
}

fn mark_browser_used() {
    let (start, last) = idle_clock();
    let now = start.elapsed().as_secs();
    last.store(now, std::sync::atomic::Ordering::Relaxed);
}

fn browser_idle_for() -> std::time::Duration {
    let (start, last) = idle_clock();
    let last = last.load(std::sync::atomic::Ordering::Relaxed);
    std::time::Duration::from_secs(start.elapsed().as_secs().saturating_sub(last))
}

/// Close the shared browser once it has sat idle for [`BROWSER_IDLE_LIMIT`].
fn start_idle_watchdog() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let mut guard = shared_browser().lock().await;
            if guard.is_some() && browser_idle_for() >= BROWSER_IDLE_LIMIT {
                tracing::debug!("ui_browser: closing the idle browser");
                guard.take();
            }
        }
    });
}

const NO_PAGE: &str =
    "No page is open. Start the dev server, then call ui_browser with action `open` and its URL.";

/// Run one `ui_browser` action against the process-wide headless browser.
pub async fn run_browser(input: UiBrowserInput) -> String {
    let action = input.action.trim().to_ascii_lowercase();
    let mut guard = shared_browser().lock().await;
    mark_browser_used();
    if action == "close" {
        return match guard.take() {
            Some(browser) => {
                drop(browser);
                "Browser closed.".to_string()
            }
            None => "No browser was open.".to_string(),
        };
    }
    let element = input
        .element
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty());
    let result: Result<String, String> = async {
        match action.as_str() {
            "open" => {
                let url = super::browser::normalize_url(input.url.as_deref().unwrap_or(""))?;
                if guard.is_none() {
                    let executable = super::browser::find_browser()
                        .ok_or_else(super::browser::no_browser_message)?;
                    *guard = Some(super::browser::UiBrowser::launch(&executable).await?);
                    start_idle_watchdog();
                }
                let browser = guard.as_mut().ok_or(NO_PAGE)?;
                browser.open(&url).await?;
                browser.snapshot().await
            }
            "snapshot" => guard.as_mut().ok_or(NO_PAGE)?.snapshot().await,
            "click" => {
                let element = element.ok_or("`ref` is required for click")?;
                let browser = guard.as_mut().ok_or(NO_PAGE)?;
                browser.click(element).await?;
                Ok(format!("Clicked {element}.\n{}", browser.snapshot().await?))
            }
            "type" => {
                let element = element.ok_or("`ref` is required for type")?;
                let text = input.text.as_deref().ok_or("`text` is required for type")?;
                let browser = guard.as_mut().ok_or(NO_PAGE)?;
                browser.type_text(element, text, input.clear).await?;
                Ok(format!(
                    "Typed into {element}.\n{}",
                    browser.snapshot().await?
                ))
            }
            "press" => {
                let key = super::browser::parse_key(input.key.as_deref().unwrap_or(""))?;
                let browser = guard.as_mut().ok_or(NO_PAGE)?;
                browser.press(element, &key).await?;
                Ok(format!(
                    "Pressed {}.\n{}",
                    input.key.as_deref().unwrap_or_default().trim(),
                    browser.snapshot().await?
                ))
            }
            other => Err(format!(
                "unknown action `{other}`; use open, snapshot, click, type, press or close"
            )),
        }
    }
    .await;
    match result {
        Ok(text) => text,
        Err(error) => {
            // A dead socket or browser can't be reused; the next `open` starts a new one.
            let lower = error.to_ascii_lowercase();
            if lower.contains("websocket closed") || lower.contains("timed out") {
                guard.take();
            }
            format!("ui_browser {action} failed: {error}")
        }
    }
}

/// Register the UI tools on `bridge` when they are missing.
pub fn register_all(
    bridge: &crate::bridge::ToolBridge,
) -> Result<(), supercharge_tool_runtime::ToolError> {
    let toolset = bridge.toolset();
    if toolset.get_tool_metadata(UI_VERIFY).is_none() {
        bridge.register_mcp_tools(UI_VERIFY.to_owned(), UiVerifyImpl, None)?;
    }
    if toolset.get_tool_metadata(UI_BROWSER).is_none() {
        bridge.register_mcp_tools(UI_BROWSER.to_owned(), UiBrowserImpl, None)?;
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
        assert_eq!(UiBrowserImpl.id().to_string(), UI_BROWSER);
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
    async fn browser_actions_need_an_open_page_and_valid_input() {
        let run = |action: &str| {
            run_browser(UiBrowserInput {
                action: action.into(),
                ..Default::default()
            })
        };
        assert!(run("dance").await.contains("unknown action `dance`"));
        assert!(run("open").await.contains("`url` is required"));
        assert!(run("press").await.contains("`key` is required"));
        assert!(run("click").await.contains("`ref` is required"));
        let snapshot = run("snapshot").await;
        assert!(
            snapshot.contains("No page is open") || snapshot.starts_with("Page:"),
            "{snapshot}"
        );
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
