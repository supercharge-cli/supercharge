//! Built-in Supercharge UI support: frontend project detection for the bundled UI SDK skill and
//! the `ui_verify` tool behind the design verification loop.

pub mod browser;
pub mod project;
pub mod tools;
pub mod verify;

pub use project::{Framework, FrontendProject, PackageManager, ProjectCommand, detect};
pub use tools::{
    UI_BROWSER, UI_TOOL_NAMES, UI_VERIFY, UiBrowserImpl, UiVerifyImpl,
    register_all as register_ui_tools, unregister_all as unregister_ui_tools,
};
