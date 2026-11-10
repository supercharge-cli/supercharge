//! Local-only coverage and duplication analysis backing the `/codecov` slash command.
//!
//! Everything here runs on the local machine with capped parallelism and no
//! network access: coverage tools already configured in the repo are executed
//! in place, and the duplication scan is a self-contained token-window pass.

mod coverage;
mod detect;
mod dup;
mod report;

pub use coverage::{
    CoverageSummary, CoverageTool, detect_coverage_tool, parse_coverage, run_coverage,
};
pub use detect::ProjectKind;
pub use dup::{DupBlock, DupLocation, MIN_BLOCK_LINES, scan_duplication};
pub use report::{Report, analyze};
