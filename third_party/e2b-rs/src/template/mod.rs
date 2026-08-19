//! Template builder foundation — ready-check command generators, build-log
//! types, build-status/tag wrappers, and builder data types.
//!
//! This module provides:
//! - [`ReadyCmd`] and the five free functions that produce POSIX shell command
//!   strings used to signal that a sandbox is ready to serve traffic.
//! - [`LogEntry`] / [`LogEntryLevel`] — structured log entries emitted during
//!   a template build (ANSI-stripped).
//! - [`BuildStatus`], [`BuildStatusReason`], [`TemplateTag`], and
//!   [`TemplateBuildStatusResponse`] — status/tag wrapper types that map the
//!   generated API schema types to an ergonomic public API.
//! - [`BuildInfo`] — result of a build-trigger call, mapped from the
//!   internal `TemplateRequestResponseV3` wire type.
//! - [`InstructionType`], [`Instruction`], [`CopyItem`] — builder data types
//!   consumed by Plans 5c/5d to construct and serialize template build steps.
//!
//! HTTP calls and full build orchestration arrive in later milestones.

pub(crate) mod archive;
pub(crate) mod build_api;
pub mod builder;
pub(crate) mod dockerfile;
pub(crate) mod files;
pub mod handle;
pub mod log;
pub mod readycmd;
pub mod tags;
pub mod types;
pub(crate) mod upload;

pub use builder::{
    AptInstallOpts, BuildOptions, BunInstallOpts, CopyOpts, GitCloneOpts, MakeDirOpts,
    MakeSymlinkOpts, NpmInstallOpts, PipInstallOpts, RegistryConfig, RemoveOpts, RenameOpts,
    RunCmdOpts, Template,
};
pub use handle::BuildHandle;
pub use log::{LogEntry, LogEntryLevel};
pub use readycmd::{
    ReadyCmd, wait_for_file, wait_for_port, wait_for_process, wait_for_timeout, wait_for_url,
};
pub use tags::{TemplateApiOpts, TemplateTagInfo};
pub use types::{
    BuildInfo, BuildStatus, BuildStatusReason, CopyItem, Instruction, InstructionType,
    TemplateBuildStatusResponse, TemplateTag,
};
