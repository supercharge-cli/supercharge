//! Supercharge `/no-mistake` validation gate.
//!
//! Host-enforced pipeline that reviews, tests, and lints committed work in an
//! isolated git worktree so the user's checkout is left untouched. Publish
//! steps (documentation, push, pull request, and CI) are wired by later
//! modules and are skipped on a local-only run.

pub mod advisor;
pub mod args;
pub mod config;
pub mod findings;
pub mod gitops;
pub mod pipeline;
pub mod prompts;
pub mod report;

pub use advisor::{Advisor, ScriptedAdvisor, StaticAdvisor};
pub use args::{GateRequest, OutputKind, ParseError, parse_invocation, parse_tokens};
pub use config::NoMistakeConfig;
pub use findings::{Finding, FindingAction, Severity};
pub use pipeline::{GateError, run, run_with_home};
pub use report::{GateReport, Outcome, StepName, StepStatus, exit_code};

use std::path::Path;

/// Run the gate in `cwd` using configuration loaded from disk and the default advisor.
pub fn run_in_cwd(request: GateRequest) -> Result<GateReport, GateError> {
    let cwd =
        std::env::current_dir().map_err(|e| GateError::io("read the current directory", e))?;
    run_in_dir(&cwd, request)
}

/// Run the gate with `dir` as the starting working tree.
pub fn run_in_dir(dir: &Path, request: GateRequest) -> Result<GateReport, GateError> {
    let config = NoMistakeConfig::load_from_disk(dir);
    let advisor = advisor::StaticAdvisor;
    run(dir, request, config, &advisor)
}
