//! # e2b-rs
//!
//! Rust SDK for [E2B](https://e2b.dev) — cloud sandboxes for AI agents. A 1:1
//! port of the official JavaScript SDK with an idiomatic async API.
//!
//! This crate is built in milestones. This release provides the **foundation
//! layer**: configuration, errors, logging, pagination state, and URL
//! signatures. Sandbox creation, command execution, and the filesystem API
//! arrive in later milestones.
//!
//! ## Creating a sandbox
//!
//! ```no_run
//! # async fn run() -> e2b_rs::Result<()> {
//! use e2b_rs::Sandbox;
//!
//! let sandbox = Sandbox::create().template("base").await?;
//! let info = sandbox.get_info().await?;
//! assert_eq!(info.state, e2b_rs::SandboxState::Running);
//! sandbox.kill().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Pausing, metrics, and snapshots
//!
//! ```no_run
//! # async fn run() -> e2b_rs::Result<()> {
//! use e2b_rs::Sandbox;
//! let sandbox = Sandbox::create().template("base").await?;
//! let metrics = sandbox.get_metrics().await?;
//! println!("{} samples", metrics.len());
//! let snap = sandbox.create_snapshot(Some("nightly".to_string())).await?;
//! println!("snapshot {}", snap.snapshot_id);
//! sandbox.pause().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Files
//!
//! ```no_run
//! # async fn run() -> e2b_rs::Result<()> {
//! use e2b_rs::Sandbox;
//! let sandbox = Sandbox::create().template("base").await?;
//! sandbox.files().write("/tmp/hello.txt", b"hi".to_vec(), Default::default()).await?;
//! let text = sandbox.files().read("/tmp/hello.txt", None).await?;
//! assert_eq!(text, "hi");
//! let mut watch = sandbox.files().watch_dir("/tmp", Default::default()).await?;
//! while let Some(event) = watch.next().await {
//!     println!("{:?} {}", event.r#type, event.name);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Commands & PTY
//!
//! ```no_run
//! # async fn run() -> e2b_rs::Result<()> {
//! use e2b_rs::Sandbox;
//! let sandbox = Sandbox::create().template("base").await?;
//! let result = sandbox.commands().run("echo hello", Default::default()).await?;
//! println!("exit {}: {}", result.exit_code, result.stdout);
//!
//! let mut cmd = sandbox.commands().start("sleep 1; echo done", Default::default()).await?;
//! while let Some(out) = cmd.next().await {
//!     if let e2b_rs::CommandOutput::Stdout(bytes) = out {
//!         print!("{}", String::from_utf8_lossy(&bytes));
//!     }
//! }
//! let _ = cmd.wait().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Git
//!
//! ```no_run
//! # async fn run() -> e2b_rs::Result<()> {
//! use e2b_rs::Sandbox;
//! let sandbox = Sandbox::create().template("base").await?;
//! sandbox.git().clone("https://github.com/owner/repo.git", Default::default()).await?;
//! let status = sandbox.git().status("/home/user/repo", None).await?;
//! println!("{:?}", status.current_branch);
//! # Ok(())
//! # }
//! ```
//!
//! ## Volumes
//!
//! ```no_run
//! # async fn run() -> e2b_rs::Result<()> {
//! use e2b_rs::Volume;
//! let volume = Volume::create("my-data", Default::default()).await?;
//! volume.write_file("/hello.txt", b"hi".to_vec(), Default::default()).await?;
//! let text = volume.read_file("/hello.txt").await?;
//! println!("{text}");
//! # Ok(())
//! # }
//! ```
//!
//! ## Templates
//!
//! Build a custom sandbox image from a Python base, copy in a requirements
//! file, install dependencies, and start your app — all through a fluent
//! builder chain:
//!
//! ```no_run
//! # async fn run() -> e2b_rs::Result<()> {
//! use e2b_rs::Template;
//! // Start with a Python 3.12 base image.
//! let template = Template::new().from_python_image("3.12");
//! // copy() returns Result<Template>, so break the chain here with `?`.
//! let template = template.copy("requirements.txt", "/app/", Default::default())?;
//! // Remaining methods all return Template — chain them freely.
//! let template = template
//!     .run_cmd("pip install -r /app/requirements.txt", Default::default())
//!     .set_workdir("/app")
//!     .set_start_cmd("python app.py", e2b_rs::wait_for_timeout(20_000));
//! // Trigger the build and stream log entries until it completes.
//! let mut build = template.build("my-app", Default::default()).await?;
//! while let Some(log) = build.next().await {
//!     println!("{log}");
//! }
//! let info = build.wait().await?;
//! println!("built template {}", info.template_id);
//! # Ok(())
//! # }
//! ```
//!
//! ## Resolving configuration
//!
//! ```
//! use e2b_rs::{ConnectionConfig, ConnectionConfigOpts};
//!
//! let config = ConnectionConfig::new(ConnectionConfigOpts {
//!     domain: Some("e2b.app".to_string()),
//!     ..Default::default()
//! });
//! assert_eq!(config.domain, "e2b.app");
//! ```

pub(crate) mod connect;

pub(crate) mod http;

pub(crate) mod envd;

pub mod volume;

pub mod template;

pub use template::{
    AptInstallOpts, BuildHandle, BuildInfo, BuildOptions, BuildStatus, BuildStatusReason,
    BunInstallOpts, CopyItem, CopyOpts, Instruction, InstructionType, LogEntry, LogEntryLevel,
    MakeDirOpts, MakeSymlinkOpts, NpmInstallOpts, PipInstallOpts, ReadyCmd, RegistryConfig,
    RemoveOpts, RenameOpts, RunCmdOpts, Template, TemplateApiOpts, TemplateBuildStatusResponse,
    TemplateTag, TemplateTagInfo, wait_for_file, wait_for_port, wait_for_process, wait_for_timeout,
    wait_for_url,
};
// Note: `template::GitCloneOpts` is accessible via `e2b_rs::template::GitCloneOpts` to avoid
// conflict with `sandbox::GitCloneOpts` which is re-exported as `e2b_rs::GitCloneOpts`.

pub(crate) mod api;

mod utils;

pub mod errors;

pub use errors::{Error, Result};

pub mod logs;

pub use logs::{Logger, NoopLogger};

pub mod connection_config;

pub use connection_config::{
    ConnectionConfig, ConnectionConfigOpts, DEFAULT_SANDBOX_TIMEOUT_MS, REQUEST_TIMEOUT_MS,
};

pub mod sandbox;

pub use sandbox::signature::{Signature, SignatureOperation, get_signature, get_signature_now};
pub use sandbox::{
    CommandHandle, CommandOutput, CommandResult, CommandStartOpts, Commands, EntryInfo, FileType,
    Filesystem, FilesystemEvent, FilesystemEventType, FsWriteOpts, Git, GitAddOpts, GitBranches,
    GitCloneOpts, GitCommitOpts, GitConfigOpts, GitConfigScope, GitDangerouslyAuthenticateOpts,
    GitDeleteBranchOpts, GitFileStatus, GitInitOpts, GitPullOpts, GitPushOpts, GitRemoteAddOpts,
    GitRequestOpts, GitResetMode, GitResetOpts, GitRestoreOpts, GitStatus, GitStatusLabel,
    NetworkRule, ProcessInfo, Pty, PtyCreateOpts, PtySize, Sandbox, SandboxConnectBuilder,
    SandboxConnectOpts, SandboxCreateBuilder, SandboxCreateOpts, SandboxInfo, SandboxListOpts,
    SandboxMetrics, SandboxNetworkUpdate, SandboxPaginator, SandboxState, SandboxUrlOpts,
    SnapshotInfo, SnapshotListOpts, SnapshotPaginator, WatchHandle, WatchOpts, WriteEntry,
    WriteInfo,
};

pub mod paginator;

pub use paginator::PaginationState;

pub use volume::{
    Volume, VolumeAndToken, VolumeEntryStat, VolumeFileType, VolumeInfo, VolumeListOpts,
    VolumeMakeDirOpts, VolumeMetadataOpts, VolumeOpts, VolumeReadOpts, VolumeWriteOpts,
};
