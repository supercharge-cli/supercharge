//! Best-of-N: run the same task N times in isolated worktrees, grade every
//! attempt, and select the winning patch.

use crate::dataset::Task;
use crate::grader::Grader;
use crate::metrics::Grade;
use crate::runner::{AttemptRecord, Runner};
use crate::{EvalConfig, EvalError};

/// One attempt plus its grade.
#[derive(Debug, Clone)]
pub struct GradedAttempt {
    pub record: AttemptRecord,
    pub grade: Grade,
}

/// Selection policy for best-of-N.
pub trait Judge: Send + Sync {
    /// Pick the winning attempt index, or `None` to decline.
    fn select(&self, task: &Task, attempts: &[GradedAttempt]) -> Option<usize>;
}

/// Deterministic default judge: the first attempt whose tests pass wins;
/// when no attempt passes, the attempt with the fewest changed lines wins
/// (ties broken by attempt order) so a judge pass can still review the
/// smallest candidate.
pub struct FewestChangesJudge;

impl Judge for FewestChangesJudge {
    fn select(&self, _task: &Task, attempts: &[GradedAttempt]) -> Option<usize> {
        if attempts.is_empty() {
            return None;
        }
        if let Some(index) = attempts.iter().position(|a| a.grade.passed()) {
            return Some(index);
        }
        attempts
            .iter()
            .enumerate()
            .min_by_key(|(index, attempt)| (changed_lines(&attempt.record.diff), *index))
            .map(|(index, _)| index)
    }
}

/// Count `+`/`-` payload lines in a unified diff (header lines excluded).
pub fn changed_lines(diff: &str) -> usize {
    diff.lines()
        .filter(|line| {
            (line.starts_with('+') && !line.starts_with("+++"))
                || (line.starts_with('-') && !line.starts_with("---"))
        })
        .count()
}

#[derive(Debug, Clone)]
pub struct BestOfOutcome {
    /// ID of the persisted run (find artifacts under `<out_dir>/<run_id>/`).
    pub run_id: String,
    pub task_id: String,
    /// Index of the selected attempt, if any.
    pub winner: Option<usize>,
    /// Patch of the selected attempt.
    pub winner_diff: Option<String>,
    pub attempts: Vec<GradedAttempt>,
    /// True when every attempt failed grading and the judge ranked candidates
    /// instead of selecting a verified pass.
    pub judged_without_pass: bool,
}

/// Run `n` attempts for `task` and select the winner. The run is persisted
/// under the config's `out_dir` like any other run, so attempts remain
/// inspectable.
pub async fn best_of_n(
    config: &EvalConfig,
    task: &Task,
    n: usize,
    judge: &dyn Judge,
) -> Result<BestOfOutcome, EvalError> {
    let runner = Runner::new(config.clone());
    let run = runner
        .run_dataset(std::slice::from_ref(task), n.max(1))
        .await?;
    let task_record = run
        .tasks
        .first()
        .ok_or_else(|| EvalError::RunRecord("run produced no task record".to_string()))?;

    let grader = Grader::default();
    let mut attempts = Vec::new();
    for record in &task_record.attempts {
        let grade = grader.grade(task, record).await;
        attempts.push(GradedAttempt {
            record: record.clone(),
            grade,
        });
    }

    let all_passed = attempts.iter().any(|a| a.grade.passed());
    let winner = judge.select(task, &attempts);
    Ok(BestOfOutcome {
        run_id: run.run_id,
        task_id: task.task_id.clone(),
        winner_diff: winner
            .and_then(|i| attempts.get(i))
            .map(|a| a.record.diff.clone()),
        winner,
        judged_without_pass: !all_passed,
        attempts,
    })
}

#[cfg(test)]
mod tests {
    use super::{FewestChangesJudge, Judge, changed_lines};
    use crate::dataset::Task;
    use crate::metrics::Grade;

    fn graded(diff: &str, passed: bool) -> super::GradedAttempt {
        super::GradedAttempt {
            record: crate::runner::AttemptRecord {
                task_id: "t".into(),
                attempt: 0,
                exit_code: 0,
                timed_out: false,
                diff: diff.into(),
                output_tail: String::new(),
                wall_ms: 1,
            },
            grade: if passed {
                Grade::Pass
            } else {
                Grade::FailTests {
                    exit_code: 1,
                    tail: String::new(),
                }
            },
        }
    }

    #[test]
    fn judge_prefers_first_pass_then_smallest_diff() {
        let task = Task {
            task_id: "t".into(),
            instructions: String::new(),
            repo_path: std::path::PathBuf::from("."),
            base_commit: None,
            test_cmd: None,
            timeout_seconds: 1,
            env: Default::default(),
        };
        let attempts = vec![
            graded(
                "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-aa\n+bbbb\n+cccc\n+dddd\n",
                false,
            ),
            graded("--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+bb\n", false),
        ];
        assert_eq!(FewestChangesJudge.select(&task, &attempts), Some(1));

        let with_pass = vec![attempts[0].clone(), {
            let mut g = graded("--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+bb\n", true);
            g.record.attempt = 1;
            g
        }];
        assert_eq!(FewestChangesJudge.select(&task, &with_pass), Some(1));

        assert_eq!(FewestChangesJudge.select(&task, &[]), None);
    }

    #[test]
    fn changed_lines_ignores_headers() {
        let diff = "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n+new2\n";
        assert_eq!(changed_lines(diff), 3);
    }
}
