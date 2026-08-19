//! Sandbox APIs. The full sandbox lifecycle and I/O modules arrive in later
//! milestones; this milestone provides URL signatures.

pub(crate) mod api;
pub(crate) mod commands;
pub(crate) mod filesystem;
pub(crate) mod git;
pub(crate) mod mcp_gen;
pub(crate) mod network;
pub mod opts;
pub(crate) mod paginator;
pub(crate) mod pty;
#[allow(clippy::module_inception)] // `sandbox.rs` inside the `sandbox` module is intentional
pub(crate) mod sandbox;
pub mod signature;
pub(crate) mod snapshot_paginator;
pub mod types;

pub use commands::{
    CommandHandle, CommandOutput, CommandResult, CommandStartOpts, Commands, ProcessInfo,
};
pub use filesystem::{
    EntryInfo, FileType, Filesystem, FilesystemEvent, FilesystemEventType, FsWriteOpts,
    WatchHandle, WatchOpts, WriteEntry, WriteInfo,
};
pub use git::{
    Git, GitAddOpts, GitBranches, GitCloneOpts, GitCommitOpts, GitConfigOpts, GitConfigScope,
    GitDangerouslyAuthenticateOpts, GitDeleteBranchOpts, GitFileStatus, GitInitOpts, GitPullOpts,
    GitPushOpts, GitRemoteAddOpts, GitRequestOpts, GitResetMode, GitResetOpts, GitRestoreOpts,
    GitStatus, GitStatusLabel,
};
pub use network::{NetworkRule, SandboxNetworkUpdate};
pub use opts::{
    SandboxConnectOpts, SandboxCreateOpts, SandboxListOpts, SandboxUrlOpts, SnapshotListOpts,
};
pub use paginator::SandboxPaginator;
pub use pty::{Pty, PtyCreateOpts, PtySize};
pub use sandbox::{Sandbox, SandboxConnectBuilder, SandboxCreateBuilder};
pub use snapshot_paginator::SnapshotPaginator;
pub use types::{SandboxInfo, SandboxMetrics, SandboxState, SnapshotInfo};
