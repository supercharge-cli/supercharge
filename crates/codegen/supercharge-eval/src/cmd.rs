//! Ready-made entry points for the `supercharge eval` CLI subcommand.

use std::path::{Path, PathBuf};

use crate::bestof::{BestOfOutcome, FewestChangesJudge, best_of_n};
use crate::dataset::{Task, load_dataset};
use crate::grader::evaluate_run;
use crate::metrics::{RunSummary, report_markdown, summarize};
use crate::runner::Runner;
use crate::{EvalConfig, EvalError};

/// Run the agent over a dataset and grade the results. Artifacts are written
/// to `<config.out_dir>/<run_id>/`: `run.json`, `grades.json`, `report.md`.
pub async fn eval_run(
    config: &EvalConfig,
    dataset_path: &Path,
    attempts: usize,
) -> Result<RunSummary, EvalError> {
    let tasks = load_dataset(dataset_path)?;
    let runner = Runner::new(config.clone());
    let run = runner.run_dataset(&tasks, attempts.max(1)).await?;

    let run_dir = config.out_dir.join(&run.run_id);
    let summaries = evaluate_run(&run).await?;
    let summary = summarize(&run.run_id, &summaries, &[1, 2, 3, 5]);
    std::fs::write(
        run_dir.join("grades.json"),
        serde_json::to_vec_pretty(&summaries).map_err(|e| EvalError::RunRecord(e.to_string()))?,
    )?;
    std::fs::write(
        run_dir.join("report.md"),
        report_markdown(&summary, &summaries),
    )?;
    Ok(summary)
}

/// Run `n` attempts of one task and select the winning patch; the full
/// per-attempt record is persisted like a dataset run.
pub async fn eval_best_of(
    config: &EvalConfig,
    task: &Task,
    n: usize,
) -> Result<(BestOfOutcome, PathBuf), EvalError> {
    let outcome = best_of_n(config, task, n, &FewestChangesJudge).await?;
    let run_dir = config.out_dir.join(&outcome.run_id);
    let mut patch_path = run_dir.join("winner.patch");
    let winner_diff = outcome.winner_diff.clone().unwrap_or_default();
    if winner_diff.is_empty() {
        patch_path = run_dir.join("winner.patch.absent");
    } else {
        std::fs::write(&patch_path, &winner_diff)?;
    }
    Ok((outcome, patch_path))
}
