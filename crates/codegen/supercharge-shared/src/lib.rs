//! Shared utilities used by both `supercharge-shell` and its downstream clients
//! (e.g. `supercharge-pager-render`). This crate sits upstream of `supercharge-shell`
//! so it must never depend on it.

pub mod clipboard;
pub mod placeholder_images;
pub mod session;
pub mod stderr;
pub mod ui_config;
