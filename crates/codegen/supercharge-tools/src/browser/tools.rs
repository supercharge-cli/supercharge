//! Browser tool helpers. Full agent ToolKind registration is gated on install;
//! callers use [`crate::browser::BrowserRuntime`] directly for CDP actions.

use super::install::is_installed;
use super::runtime::BrowserRuntime;

/// Whether browser tools should be offered to the agent.
pub fn browser_tools_available() -> bool {
    is_installed()
}

/// Register browser tools into the process tool-pack list when the binary is present.
/// Currently records availability only; CDP actions are exposed via [`BrowserRuntime`]
/// and the `supercharge browser` CLI until a dedicated ToolKind lands.
pub fn register_browser_tools_if_installed() {
    if !is_installed() {
        tracing::debug!("browser binary not installed; browser tools stay unregistered");
        return;
    }
    tracing::info!("browser binary present; CDP runtime available via BrowserRuntime::global()");
    // Touch the global so Drop/shutdown hooks can find it later.
    let _ = BrowserRuntime::global();
}

/// Convenience: open URL, return visible text (for tests and scripting).
pub async fn open_and_read_text(url: &str) -> Result<String, super::cdp::CdpError> {
    let runtime = BrowserRuntime::global();
    let mut session = runtime.session(url).await?;
    // Give the page a brief moment to settle.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    session.page_text().await
}
