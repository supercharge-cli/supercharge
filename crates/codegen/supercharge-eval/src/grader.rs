//! Grades attempt patches in a clean checkout of the base commit.

use std::time::Duration;

use crate::dataset::Task;
use crate::metrics::{AttemptGrade, Grade, TaskSummary};
use crate::runner::{RunRecord, create_worktree, prune_worktrees};

/// Grades patches. Each grade runs the task's test command in a fresh worktree
/// at the base commit with the patch applied, so attempts are judged on
/// self-contained patches rather than on whatever state the agent left behind.
pub struct Grader {
    /// Cap applied to the test command on top of the task's own timeout.
    pub test_timeout_seconds: u64,
}

impl Default for Grader {
    fn default() -> Self {
        Self {
            test_timeout_seconds: 600,
        }
    }
}

impl Grader {
    /// Grade one attempt's patch against `task`.
    pub async fn grade(&self, task: &Task, record: &crate::runner::AttemptRecord) -> Grade {
        let temp = match tempfile::TempDir::new() {
            Ok(temp) => temp,
            Err(e) => {
                return Grade::Error {
                    detail: format!("tempdir: {e}"),
                };
            }
        };
        let grade = self.grade_in(temp.path(), task, record).await;
        drop(temp);
        prune_worktrees(&task.repo_path);
        grade
    }

    async fn grade_in(
        &self,
        workdir: &std::path::Path,
        task: &Task,
        record: &crate::runner::AttemptRecord,
    ) -> Grade {
        if record.timed_out {
            return Grade::Timeout;
        }
        if record.diff.is_empty() && record.exit_code != 0 {
            return Grade::Error {
                detail: format!("agent exited {} without producing a diff", record.exit_code),
            };
        }

        let checkout = workdir.join("checkout");
        if let Err(e) = create_worktree(&task.repo_path, task.base_commit.as_deref(), &checkout) {
            return Grade::Error {
                detail: e.to_string(),
            };
        }

        if !record.diff.is_empty()
            && let Err(e) = apply_patch(&checkout, &record.diff)
        {
            return Grade::FailApply { detail: e };
        }

        let Some(test_cmd) = task.test_cmd.as_deref() else {
            return Grade::Pass;
        };

        let mut command = tokio::process::Command::new("sh");
        command.arg("-c").arg(test_cmd).current_dir(&checkout);
        for (key, value) in &task.env {
            command.env(key, value);
        }
        command.kill_on_drop(true);
        let waited = command.output();
        match tokio::time::timeout(Duration::from_secs(self.test_timeout_seconds), waited).await {
            Ok(Ok(output)) if output.status.success() => Grade::Pass,
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let combined = if stderr.is_empty() {
                    stdout.into_owned()
                } else {
                    format!("{stdout}\n{stderr}")
                };
                Grade::FailTests {
                    exit_code: output.status.code().unwrap_or(-1),
                    tail: crate::tail(&combined, 8_000),
                }
            }
            Ok(Err(e)) => Grade::Error {
                detail: format!("test spawn: {e}"),
            },
            Err(_) => Grade::Timeout,
        }
    }
}

/// `git apply` a patch from memory into `dir`. The patch is handed over via a
/// temp file so the git child can be spawned+reaped in one call.
fn apply_patch(dir: &std::path::Path, diff: &str) -> Result<(), String> {
    let patch = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    std::fs::write(patch.path(), diff).map_err(|e| format!("writing patch: {e}"))?;

    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["apply", "--whitespace=nowarn"])
        .arg(patch.path())
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Grade every attempt in a saved run. Attempts are graded sequentially; the
/// test commands themselves are the bottleneck, not the grading overhead.
pub async fn evaluate_run(run: &RunRecord) -> Result<Vec<TaskSummary>, crate::EvalError> {
    let grader = Grader::default();
    let mut summaries = Vec::new();
    for task_record in &run.tasks {
        let mut attempts = Vec::new();
        for record in &task_record.attempts {
            let grade = grader.grade(&task_record.task, record).await;
            attempts.push(AttemptGrade {
                attempt: record.attempt,
                grade,
                wall_ms: record.wall_ms,
            });
        }
        summaries.push(TaskSummary {
            task_id: task_record.task.task_id.clone(),
            attempts,
        });
    }
    Ok(summaries)
}
