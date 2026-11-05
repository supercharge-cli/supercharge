//! Offline eval harness and best-of-N runner for the Supercharge agent.
//!
//! The harness treats the agent as a subprocess. Each attempt runs the
//! configured agent command in an isolated `git worktree`, captures the final
//! patch, and grades it by applying it to a clean checkout of the base commit
//! and running the task's test command. Runs are persisted as JSON so they can
//! be re-graded later without invoking the agent again.
//!
//! See `docs/eval-harness.md` for the dataset format and CLI usage.

pub mod bestof;
pub mod cmd;
pub mod dataset;
pub mod grader;
pub mod metrics;
pub mod presets;
pub mod replay;
pub mod runner;

pub use dataset::Task;
pub use presets::{Preset, load_preset, materialize as materialize_preset};
pub use grader::Grader;
pub use metrics::{AttemptGrade, Grade, RunSummary, TaskSummary};
pub use runner::{AttemptRecord, RunRecord, Runner};

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("dataset error: {0}")]
    Dataset(String),
    #[error("git `{args}` failed: {stderr}")]
    Git { args: String, stderr: String },
    #[error("agent spawn failed for task {task_id} attempt {attempt}: {source}")]
    AgentSpawn {
        task_id: String,
        attempt: usize,
        source: std::io::Error,
    },
    #[error("run record error: {0}")]
    RunRecord(String),
    #[error("invalid path: {0}")]
    Path(String),
}

/// How the harness invokes the agent for one attempt. `{prompt}` in `args` is
/// replaced with the task instructions; the process runs with its working
/// directory set to the attempt worktree. The default matches the headless
/// one-shot mode of the Supercharge binary.
#[derive(Debug, Clone)]
pub struct AgentCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl Default for AgentCommand {
    fn default() -> Self {
        Self {
            program: "supercharge".to_string(),
            args: vec!["--single".to_string(), "{prompt}".to_string()],
        }
    }
}

impl AgentCommand {
    /// Build from a program name and a shell-style args string; `{prompt}` is
    /// replaced with task instructions at spawn time.
    pub fn parse(program: impl Into<String>, args: &str) -> Option<Self> {
        Some(Self {
            program: program.into(),
            args: shlex::split(args)?,
        })
    }
}

/// Harness configuration shared by the runner, grading, and best-of-N.
#[derive(Debug, Clone)]
pub struct EvalConfig {
    pub agent: AgentCommand,
    /// Directory holding run artifacts: `<out_dir>/<run_id>/…`.
    pub out_dir: PathBuf,
    /// Maximum attempts (or tasks across a dataset) executing concurrently.
    pub max_parallel: usize,
}

impl Default for EvalConfig {
    fn default() -> Self {
        Self {
            agent: AgentCommand::default(),
            out_dir: PathBuf::from(".supercharge-eval/runs"),
            max_parallel: 4,
        }
    }
}

/// Last `max_chars` characters of a captured output stream; tails are more
/// useful than heads for test failures.
pub(crate) fn tail(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    s.chars().skip(count - max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::tail;

    #[test]
    fn tail_keeps_short_strings_and_trims_long_ones() {
        assert_eq!(tail("abc", 10), "abc");
        let long = "x".repeat(100);
        assert_eq!(tail(&long, 10), long[90..]);
    }
}
