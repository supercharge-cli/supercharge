//! Grading outcomes and run summaries (pass@k, wall-time stats, report).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Classification of one graded attempt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "grade", rename_all = "snake_case")]
pub enum Grade {
    /// The patch applied cleanly and the test command exited zero (or the task
    /// has no test command and the patch applied).
    Pass,
    /// `git apply` rejected the patch.
    FailApply { detail: String },
    /// The patch applied but the test command failed.
    FailTests { exit_code: i32, tail: String },
    /// The agent or the test command ran past the task timeout.
    Timeout,
    /// Anything else: agent crash, spawn failure, harness failure.
    Error { detail: String },
}

impl Grade {
    pub fn passed(&self) -> bool {
        matches!(self, Grade::Pass)
    }

    pub fn name(&self) -> &'static str {
        match self {
            Grade::Pass => "pass",
            Grade::FailApply { .. } => "fail_apply",
            Grade::FailTests { .. } => "fail_tests",
            Grade::Timeout => "timeout",
            Grade::Error { .. } => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptGrade {
    pub attempt: usize,
    pub grade: Grade,
    pub wall_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub task_id: String,
    pub attempts: Vec<AttemptGrade>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: String,
    pub tasks: usize,
    /// Fraction of tasks whose first attempt passed.
    pub pass_at_1: f64,
    /// Unbiased pass@k averaged over tasks with at least `k` attempts.
    pub pass_at_k: BTreeMap<usize, f64>,
    pub median_wall_ms: u64,
    pub p90_wall_ms: u64,
    pub total_wall_ms: u64,
    pub grade_counts: BTreeMap<String, usize>,
}

/// Unbiased pass@k estimator over `n` samples with `c` passing:
/// `1 - C(n-c, k) / C(n, k)`.
pub fn pass_at_k(n: usize, c: usize, k: usize) -> f64 {
    debug_assert!(k >= 1);
    if k > n {
        return 0.0;
    }
    if c >= n {
        return 1.0;
    }
    if c == 0 {
        return 0.0;
    }
    let mut complement = 1.0f64;
    for i in 0..k {
        complement *= (n - c - i) as f64 / (n - i) as f64;
    }
    1.0 - complement
}

/// Summarize graded attempts into pass@k and wall-time statistics.
pub fn summarize(run_id: &str, tasks: &[TaskSummary], ks: &[usize]) -> RunSummary {
    let attempt_0: Vec<&AttemptGrade> = tasks
        .iter()
        .filter_map(|t| t.attempts.iter().find(|a| a.attempt == 0))
        .collect();
    let pass_at_1 = if attempt_0.is_empty() {
        0.0
    } else {
        attempt_0.iter().filter(|a| a.grade.passed()).count() as f64 / attempt_0.len() as f64
    };

    let mut pass_at_k_map = BTreeMap::new();
    for &k in ks {
        let eligible: Vec<&TaskSummary> = tasks.iter().filter(|t| t.attempts.len() >= k).collect();
        if eligible.is_empty() {
            continue;
        }
        let estimate = eligible
            .iter()
            .map(|t| {
                let n = t.attempts.len();
                let c = t.attempts.iter().filter(|a| a.grade.passed()).count();
                pass_at_k(n, c, k)
            })
            .sum::<f64>()
            / eligible.len() as f64;
        pass_at_k_map.insert(k, estimate);
    }

    let mut walls: Vec<u64> = tasks
        .iter()
        .flat_map(|t| t.attempts.iter().map(|a| a.wall_ms))
        .collect();
    walls.sort_unstable();
    let total_wall_ms = walls.iter().sum();
    let pick = |frac: f64| -> u64 {
        if walls.is_empty() {
            return 0;
        }
        let idx = ((walls.len() as f64 - 1.0) * frac).round() as usize;
        walls[idx.min(walls.len() - 1)]
    };

    let mut grade_counts: BTreeMap<String, usize> = BTreeMap::new();
    for attempt in tasks.iter().flat_map(|t| &t.attempts) {
        *grade_counts
            .entry(attempt.grade.name().to_string())
            .or_default() += 1;
    }

    RunSummary {
        run_id: run_id.to_string(),
        tasks: tasks.len(),
        pass_at_1,
        pass_at_k: pass_at_k_map,
        median_wall_ms: pick(0.5),
        p90_wall_ms: pick(0.9),
        total_wall_ms,
        grade_counts,
    }
}

/// Human-readable markdown report for a run.
pub fn report_markdown(summary: &RunSummary, tasks: &[TaskSummary]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Eval report — {}\n\n", summary.run_id));
    out.push_str(&format!("- tasks: {}\n", summary.tasks));
    out.push_str(&format!("- pass@1: {:.3}\n", summary.pass_at_1));
    for (k, v) in summary.pass_at_k.iter().skip_while(|(k, _)| **k <= 1) {
        out.push_str(&format!("- pass@{k}: {v:.3}\n"));
    }
    out.push_str(&format!(
        "- wall time: median {} ms, p90 {} ms, total {} ms\n",
        summary.median_wall_ms, summary.p90_wall_ms, summary.total_wall_ms
    ));
    out.push_str("\n| task | attempt | grade | wall_ms |\n|---|---|---|---|\n");
    for task in tasks {
        for attempt in &task.attempts {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                task.task_id,
                attempt.attempt,
                attempt.grade.name(),
                attempt.wall_ms
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{AttemptGrade, Grade, TaskSummary, pass_at_k, summarize};

    fn task(id: &str, grades: &[bool]) -> TaskSummary {
        TaskSummary {
            task_id: id.to_string(),
            attempts: grades
                .iter()
                .enumerate()
                .map(|(attempt, passed)| AttemptGrade {
                    attempt,
                    grade: if *passed {
                        Grade::Pass
                    } else {
                        Grade::FailTests {
                            exit_code: 1,
                            tail: String::new(),
                        }
                    },
                    wall_ms: 100,
                })
                .collect(),
        }
    }

    #[test]
    fn pass_at_k_estimator_matches_textbook_values() {
        assert!((pass_at_k(3, 1, 1) - (1.0 / 3.0)).abs() < 1e-9);
        // 1 - C(2,2)/C(3,2) = 1 - 1/3
        assert!((pass_at_k(3, 1, 2) - (2.0 / 3.0)).abs() < 1e-9);
        assert_eq!(pass_at_k(3, 3, 2), 1.0);
        assert_eq!(pass_at_k(3, 0, 2), 0.0);
        assert_eq!(pass_at_k(2, 1, 5), 0.0); // k > n
    }

    #[test]
    fn summarize_reports_pass_rates_and_walls() {
        let tasks = vec![task("a", &[true, false]), task("b", &[false, false])];
        let summary = summarize("run", &tasks, &[1, 2]);
        assert_eq!(summary.tasks, 2);
        assert!((summary.pass_at_1 - 0.5).abs() < 1e-9);
        // pass@2 over both eligible tasks: task a → 1.0, task b → 0.0, mean 0.5
        assert!((summary.pass_at_k[&2] - 0.5).abs() < 1e-9);
        assert_eq!(summary.grade_counts["pass"], 1);
        assert_eq!(summary.grade_counts["fail_tests"], 3);
        assert_eq!(summary.total_wall_ms, 400);
        assert_eq!(summary.median_wall_ms, 100);
    }
}
