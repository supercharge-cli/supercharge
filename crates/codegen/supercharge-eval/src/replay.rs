//! Re-grade a saved run without invoking the agent.

use std::path::Path;

use crate::EvalError;
use crate::grader::evaluate_run;
use crate::metrics::{RunSummary, report_markdown, summarize};
use crate::runner::load_run;

/// Load `<run_path>/run.json` (or a direct path to it), re-grade every stored
/// patch with the current grader, and write `grades.json` + `report.md` under
/// `<out_dir>/<run_id>-replay/`.
pub async fn replay(run_path: &Path, out_dir: &Path) -> Result<RunSummary, EvalError> {
    // Accept either the run directory or the run.json file inside it.
    let run_file = if run_path.is_dir() {
        run_path.join("run.json")
    } else {
        run_path.to_path_buf()
    };
    let run = load_run(&run_file)?;
    let summaries = evaluate_run(&run).await?;
    let summary = summarize(&run.run_id, &summaries, &[1, 2, 3, 5]);

    let out = out_dir.join(format!("{}-replay", run.run_id));
    std::fs::create_dir_all(&out)?;
    std::fs::write(
        out.join("grades.json"),
        serde_json::to_vec_pretty(&summaries).map_err(|e| EvalError::RunRecord(e.to_string()))?,
    )?;
    std::fs::write(out.join("report.md"), report_markdown(&summary, &summaries))?;
    Ok(summary)
}
