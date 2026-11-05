//! Optional local headless browser tools, backed by a downloaded browser binary
//! under the Supercharge home. Tools register only when the binary is installed;
//! the CDP server is spawned on first use and stopped when the process exits.

pub mod cdp;
pub mod install;
pub mod runtime;
pub mod tools;

pub use install::{
    BrowserInstallError, binary_path, install_latest, is_installed, status_report, uninstall,
};
pub use runtime::BrowserRuntime;
pub use tools::register_browser_tools_if_installed;
