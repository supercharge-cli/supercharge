//! Public value types for sandbox commands.

use std::collections::BTreeMap;

use crate::envd::proto::process as pb;

/// A chunk of live output from a running command.
#[derive(Debug, Clone)]
pub enum CommandOutput {
    /// Bytes written to stdout.
    Stdout(Vec<u8>),
    /// Bytes written to stderr.
    Stderr(Vec<u8>),
    /// Raw PTY output bytes (for `pty` sessions).
    Pty(Vec<u8>),
}

/// The result of a finished command.
#[derive(Debug, Clone)]
pub struct CommandResult {
    /// Process exit code (a non-zero code is NOT an SDK error — inspect it here).
    pub exit_code: i32,
    /// Error description if the process failed to run/exit cleanly.
    pub error: Option<String>,
    /// Full accumulated stdout (lossy UTF-8).
    pub stdout: String,
    /// Full accumulated stderr (lossy UTF-8).
    pub stderr: String,
}

/// Info about a running process (returned by `Commands::list`).
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    /// Process id.
    pub pid: u32,
    /// Optional process tag.
    pub tag: Option<String>,
    /// Command binary.
    pub cmd: String,
    /// Command arguments.
    pub args: Vec<String>,
    /// Environment variables.
    pub envs: BTreeMap<String, String>,
    /// Working directory, if set.
    pub cwd: Option<String>,
}

impl ProcessInfo {
    /// Map the generated proto type to the public one.
    #[allow(dead_code)] // used by Task 2+ (Commands::list)
    pub(crate) fn from_proto(p: pb::ProcessInfo) -> ProcessInfo {
        let config = p.config.unwrap_or_default();
        ProcessInfo {
            pid: p.pid,
            tag: p.tag,
            cmd: config.cmd,
            args: config.args,
            envs: config.envs.into_iter().collect(),
            cwd: config.cwd,
        }
    }
}
