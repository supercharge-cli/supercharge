#![allow(
    unused_imports,
    unused_variables,
    unused_mut,
    unreachable_code,
    dead_code
)]
#![warn(unreachable_pub)]
#[cfg(all(test, feature = "dhat-heap"))]
#[global_allocator]
static DHAT_ALLOC: dhat::Alloc = dhat::Alloc;
pub(crate) use supercharge_telemetry::unified_log;
pub use supercharge_tracing_macros::{teprintln, timed, tprintln};
pub mod agent;
pub mod auth;
pub mod builtin;
pub use supercharge_bundle as bundle;
pub mod claude_import;
pub mod claude_import_state;
pub mod cli_models;
pub mod config;
pub use supercharge_shell_base::cpu_profile;
pub use supercharge_shell_base::env;
pub mod extensions;
pub use supercharge_foreign_sessions as foreign_sessions;
pub mod heap_profile;
pub use supercharge_http as http;
pub mod inspect;
pub mod instrumentation;
pub mod leader;
pub mod managed_config;
pub mod mcp_doctor;
pub use supercharge_models as models;
pub mod plugin;
pub mod relay;
pub mod remote;
pub mod sampling;
pub mod session;
pub use supercharge_superagent as superagent;
pub mod terminal;
#[cfg(test)]
pub(crate) mod test_support;
pub mod tier;
pub mod tools;
pub mod upload;
pub mod util;
