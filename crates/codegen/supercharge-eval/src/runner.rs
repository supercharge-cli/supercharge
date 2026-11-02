//! Runs the agent over a dataset. Each attempt gets an isolated `git worktree`
//! at the task's base commit; the agent command runs there, the final patch is
//! captured, and the worktree is removed. Every run is persisted as JSON under
//! `<out_dir>/<run_id>/run.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use crate::dataset::Task;
use crate::{EvalConfig, EvalError};

/// Final state of one attempt as captured by the runner (before grading).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub task_id: String,
    pub attempt: usize,
    pub exit_code: i32,
    pub timed_out: bool,
    /// Patch produced by the agent: `git diff --cached --binary` after
    /// `git add -A`, so untracked and binary files are included.
    pub diff: String,
    pub output_tail: String,
    pub wall_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub task: Task,
    pub attempts: Vec<AttemptRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    /// Provenance of the agent invocation used for this run.
    pub agent_program: String,
    pub agent_args: Vec<String>,
    pub tasks: Vec<TaskRecord>,
}

pub struct Runner {
    config: EvalConfig,
}

impl Runner {
    pub fn new(config: EvalConfig) -> Self {
        Self { config }
    }

    /// Run `attempts_per_task` attempts for every task and persist the run.
    /// Attempts and tasks share the configured concurrency budget.
    pub async fn run_dataset(
        &self,
        tasks: &[Task],
        attempts_per_task: usize,
    ) -> Result<RunRecord, EvalError> {
        let attempts_per_task = attempts_per_task.max(1);
        let mut task_map = BTreeMap::new();
        for task in tasks {
            if task_map
                .insert(task.task_id.clone(), task.clone())
                .is_some()
            {
                return Err(EvalError::Dataset(format!(
                    "duplicate task_id `{}`",
                    task.task_id
                )));
            }
        }

        let run_id = uuid::Uuid::new_v4().to_string();
        let run_dir = self.config.out_dir.join(&run_id);
        std::fs::create_dir_all(run_dir.join("worktrees"))?;

        let semaphore = Arc::new(Semaphore::new(self.config.max_parallel.max(1)));
        let mut join = tokio::task::JoinSet::new();
        for task in tasks {
            for attempt in 0..attempts_per_task {
                let permit = semaphore
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("semaphore closed");
                join.spawn(Self::run_attempt(
                    self.config.clone(),
                    run_dir.clone(),
                    task.clone(),
                    attempt,
                    permit,
                ));
            }
        }

        let mut records = Vec::new();
        while let Some(joined) = join.join_next().await {
            records.push(
                joined.map_err(|e| EvalError::RunRecord(format!("attempt panicked: {e}")))??,
            );
        }
        records.sort_by(|a, b| a.task_id.cmp(&b.task_id).then(a.attempt.cmp(&b.attempt)));

        let mut tasks_out: Vec<TaskRecord> = Vec::new();
        for record in records {
            match tasks_out.last_mut() {
                Some(last) if last.task.task_id == record.task_id => {
                    last.attempts.push(record);
                }
                _ => tasks_out.push(TaskRecord {
                    task: task_map[&record.task_id].clone(),
                    attempts: vec![record],
                }),
            }
        }

        let run = RunRecord {
            run_id,
            agent_program: self.config.agent.program.clone(),
            agent_args: self.config.agent.args.clone(),
            tasks: tasks_out,
        };
        save_run(&run, &run_dir)?;
        Ok(run)
    }

    async fn run_attempt(
        config: EvalConfig,
        run_dir: PathBuf,
        task: Task,
        attempt: usize,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<AttemptRecord, EvalError> {
        let _ = permit; // held for the whole attempt
        let started = Instant::now();
        let worktree = run_dir
            .join("worktrees")
            .join(format!("{}-{attempt}", task.task_id));

        create_worktree(&task.repo_path, task.base_commit.as_deref(), &worktree)?;
        let agent_result = run_agent(&config.agent, &task, attempt, &worktree).await;
        let spawn_failed = matches!(agent_result, AgentRun::SpawnFailed { .. });
        let diff = if spawn_failed {
            String::new()
        } else {
            capture_diff(&worktree)?
        };
        remove_worktree(&task.repo_path, &worktree);

        Ok(match agent_result {
            AgentRun::Completed { exit_code, output } => AttemptRecord {
                task_id: task.task_id.clone(),
                attempt,
                exit_code,
                timed_out: false,
                diff,
                output_tail: output,
                wall_ms: started.elapsed().as_millis() as u64,
            },
            AgentRun::SpawnFailed { error } => AttemptRecord {
                task_id: task.task_id.clone(),
                attempt,
                exit_code: -1,
                timed_out: false,
                diff,
                output_tail: crate::tail(&error, 8_000),
                wall_ms: started.elapsed().as_millis() as u64,
            },
            AgentRun::TimedOut => AttemptRecord {
                task_id: task.task_id.clone(),
                attempt,
                exit_code: -1,
                timed_out: true,
                diff,
                output_tail: String::new(),
                wall_ms: started.elapsed().as_millis() as u64,
            },
        })
    }
}

enum AgentRun {
    Completed { exit_code: i32, output: String },
    SpawnFailed { error: String },
    TimedOut,
}

async fn run_agent(
    agent: &crate::AgentCommand,
    task: &Task,
    attempt: usize,
    cwd: &Path,
) -> AgentRun {
    let mut command = tokio::process::Command::new(&agent.program);
    for arg in &agent.args {
        command.arg(arg.replace("{prompt}", &task.instructions));
    }
    command.current_dir(cwd);
    command.env("SUPERCHARGE_EVAL_TASK_ID", &task.task_id);
    command.env("SUPERCHARGE_EVAL_ATTEMPT", attempt.to_string());
    for (key, value) in &task.env {
        command.env(key, value);
    }
    // Killing the dropped future kills the child: timed-out agents must not
    // keep consuming tokens after the attempt is over.
    command.kill_on_drop(true);

    let waited = command.output();
    match tokio::time::timeout(Duration::from_secs(task.timeout_seconds), waited).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = if stderr.is_empty() {
                stdout.into_owned()
            } else {
                format!("{stdout}\n-- stderr --\n{stderr}")
            };
            AgentRun::Completed {
                exit_code: output.status.code().unwrap_or(-1),
                output: crate::tail(&combined, 8_000),
            }
        }
        Ok(Err(source)) => AgentRun::SpawnFailed {
            error: format!("agent process failed: {source}"),
        },
        // The future owning the child is dropped here, which kills it.
        Err(_) => AgentRun::TimedOut,
    }
}

fn run_git(repo: &Path, args: &[&str]) -> Result<std::process::Output, EvalError> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(EvalError::Io)
}

fn ensure_git_success(
    repo: &Path,
    args: &[&str],
    output: &std::process::Output,
) -> Result<(), EvalError> {
    if output.status.success() {
        return Ok(());
    }
    Err(EvalError::Git {
        args: format!("(in {}) {}", repo.display(), args.join(" ")),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Create a detached worktree of `repo` at `base` (or HEAD) in `dest`.
pub fn create_worktree(repo: &Path, base: Option<&str>, dest: &Path) -> Result<(), EvalError> {
    let dest = dest
        .to_str()
        .ok_or_else(|| EvalError::Path(dest.display().to_string()))?;
    let mut args = vec!["worktree", "add", "--detach", dest];
    if let Some(base) = base {
        args.push(base);
    }
    let output = run_git(repo, &args)?;
    ensure_git_success(repo, &args, &output)
}

/// Best-effort worktree cleanup; leftover metadata is pruned.
pub fn remove_worktree(repo: &Path, dest: &Path) {
    if let Some(dest) = dest.to_str() {
        let _ = run_git(repo, &["worktree", "remove", "--force", dest]);
    }
    let _ = run_git(repo, &["worktree", "prune"]);
}

/// Drop stale worktree metadata (e.g. after a grader checkout directory was
/// deleted by its TempDir guard).
pub fn prune_worktrees(repo: &Path) {
    let _ = run_git(repo, &["worktree", "prune"]);
}

/// Capture everything the agent changed in `worktree` as one patch.
pub fn capture_diff(worktree: &Path) -> Result<String, EvalError> {
    let add = run_git(worktree, &["add", "-A"])?;
    ensure_git_success(worktree, &["add", "-A"], &add)?;
    let diff = run_git(worktree, &["diff", "--cached", "--binary"])?;
    ensure_git_success(worktree, &["diff", "--cached", "--binary"], &diff)?;
    Ok(String::from_utf8_lossy(&diff.stdout).into_owned())
}

/// Persist a run record as `<dir>/run.json`.
pub fn save_run(run: &RunRecord, dir: &Path) -> Result<(), EvalError> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("run.json");
    std::fs::write(
        path,
        serde_json::to_vec_pretty(run).map_err(|e| EvalError::RunRecord(e.to_string()))?,
    )?;
    Ok(())
}

/// Load a run record saved by [`save_run`].
pub fn load_run(path: &Path) -> Result<RunRecord, EvalError> {
    let text = std::fs::read_to_string(path)?;
    serde_json::from_str(&text)
        .map_err(|e| EvalError::RunRecord(format!("{}: {e}", path.display())))
}
