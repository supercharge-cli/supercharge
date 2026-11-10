//! Report assembly, deterministic text rendering, and the fix-phase prompt.
//!
//! The report itself is fully deterministic (native engine output). The fix
//! prompt is the only AI-facing piece: it hands the agent the exact gaps and
//! requires confirmation-scoped, test-backed changes.

use std::path::Path;

use crate::coverage::{CoverageSummary, detect_coverage_tool, parse_coverage, run_coverage};
use crate::detect::{ProjectKind, detect_project};
use crate::dup::{DupBlock, MIN_BLOCK_LINES, scan_duplication};

const MAX_REPORT_TARGETS: usize = 5;
const MAX_REPORT_BLOCKS: usize = 10;

/// Full result of a local `/codecov` run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub scope: String,
    pub kind: ProjectKind,
    pub coverage_tool: Option<&'static str>,
    pub coverage: Option<CoverageSummary>,
    pub coverage_note: Option<String>,
    pub dups: Vec<DupBlock>,
    pub dup_total_lines: usize,
}

/// Run the local checks: duplication scan plus the repo's own coverage tool
/// when one is present. Nothing is installed; a missing tool is reported as
/// a note instead.
pub fn analyze(root: &Path, scope: Option<&str>) -> Report {
    let kind = detect_project(root);
    let (dups, dup_total_lines) = scan_duplication(root, scope, MAX_REPORT_BLOCKS);

    let mut report = Report {
        scope: scope.unwrap_or("whole workspace").to_string(),
        kind,
        dups,
        dup_total_lines,
        ..Report::default()
    };

    match detect_coverage_tool(root, kind) {
        Some(tool) => match run_coverage(root, &tool) {
            Some(stdout) => {
                report.coverage_tool = Some(tool.name);
                report.coverage = Some(parse_coverage(kind, &stdout));
            }
            None => {
                report.coverage_tool = Some(tool.name);
                report.coverage_note =
                    Some(format!("{} failed; run it manually for details", tool.name));
            }
        },
        None => {
            report.coverage_note = Some(match kind {
                ProjectKind::Rust => {
                    "no coverage tool found (install cargo-llvm-cov to enable coverage)".to_string()
                }
                ProjectKind::Go => "go not found on PATH; skipping coverage".to_string(),
                ProjectKind::Python => "pytest not found on PATH; skipping coverage".to_string(),
                ProjectKind::JavaScript => {
                    "no \"coverage\" script in package.json; skipping coverage".to_string()
                }
                ProjectKind::Unknown => "unknown project type; skipping coverage".to_string(),
            });
        }
    }
    report
}

impl Report {
    /// Deterministic plain-text report for the scrollback system block.
    pub fn render(&self) -> String {
        let mut out = String::from("Codecov report (local)\n");
        out.push_str(&format!("Scope: {}\n", self.scope));

        match (&self.coverage, self.coverage_tool) {
            (Some(summary), tool) => {
                let name = tool.unwrap_or("coverage");
                match summary.overall {
                    Some(overall) => out.push_str(&format!("Coverage: {overall:.1}% ({name})\n")),
                    None => out.push_str(&format!("Coverage: ran {name}, no total parsed\n")),
                }
                let mut targets: Vec<&(String, f64)> = summary
                    .per_target
                    .iter()
                    .filter(|(_, percent)| *percent < 80.0)
                    .collect();
                targets.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                for (target, percent) in targets.iter().take(MAX_REPORT_TARGETS) {
                    out.push_str(&format!("  {target}: {percent:.1}%\n"));
                }
            }
            (None, _) => {
                if let Some(note) = &self.coverage_note {
                    out.push_str(&format!("Coverage: skipped ({note})\n"));
                }
            }
        }

        if self.dups.is_empty() {
            out.push_str(&format!(
                "Duplication: none found (blocks >= {MIN_BLOCK_LINES} lines)\n"
            ));
        } else {
            out.push_str(&format!(
                "Duplication: {} blocks, {} duplicated lines (>= {MIN_BLOCK_LINES} lines)\n",
                self.dups.len(),
                self.dup_total_lines
            ));
            for (idx, block) in self.dups.iter().enumerate().take(MAX_REPORT_BLOCKS) {
                let locations = block
                    .locations
                    .iter()
                    .map(|loc| format!("{}:{}-{}", loc.file, loc.start, loc.end))
                    .collect::<Vec<_>>()
                    .join(" <-> ");
                out.push_str(&format!(
                    "  {}. {} ({} lines)\n",
                    idx + 1,
                    locations,
                    block.lines
                ));
            }
        }
        out
    }

    /// The AI-facing fix prompt for `/codecov fix`: the agent receives the
    /// exact gaps and must write tests and dedupe only what is listed.
    pub fn fix_prompt(&self) -> String {
        let mut out = String::from(
            "Fix the code-quality gaps listed below from this local codecov report.\n\
             Only touch the listed items: write tests for the uncovered areas and \
             extract duplicated blocks into shared helpers. Run the project's test \
             suite afterwards and report what changed. Do not refactor unrelated code.\n\n",
        );
        if let Some(summary) = &self.coverage {
            if let Some(overall) = summary.overall {
                out.push_str(&format!("Coverage is {overall:.1}%.\n"));
            }
            let mut targets: Vec<&(String, f64)> = summary
                .per_target
                .iter()
                .filter(|(_, percent)| *percent < 80.0)
                .collect();
            targets.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            if !targets.is_empty() {
                out.push_str("Uncovered areas needing tests:\n");
                for (target, percent) in targets.iter().take(MAX_REPORT_TARGETS) {
                    out.push_str(&format!("- {target} at {percent:.1}%\n"));
                }
            }
        }
        if !self.dups.is_empty() {
            out.push_str("Duplicated blocks to extract:\n");
            for block in self.dups.iter().take(MAX_REPORT_TARGETS) {
                let locations = block
                    .locations
                    .iter()
                    .map(|loc| format!("{}:{}-{}", loc.file, loc.start, loc.end))
                    .collect::<Vec<_>>()
                    .join(" and ");
                out.push_str(&format!("- {locations} ({} lines)\n", block.lines));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::CoverageSummary;
    use crate::dup::{DupLocation, MIN_BLOCK_LINES};

    fn sample_report() -> Report {
        Report {
            scope: "whole workspace".to_string(),
            kind: ProjectKind::Rust,
            coverage_tool: Some("cargo-llvm-cov"),
            coverage: Some(CoverageSummary {
                overall: Some(68.8),
                per_target: vec![
                    ("src/lib.rs".to_string(), 81.7),
                    ("src/codecov/mod.rs".to_string(), 42.0),
                ],
            }),
            coverage_note: None,
            dups: vec![DupBlock {
                locations: vec![
                    DupLocation {
                        file: "src/a.rs".to_string(),
                        start: 1,
                        end: 8,
                    },
                    DupLocation {
                        file: "src/b.rs".to_string(),
                        start: 1,
                        end: 8,
                    },
                ],
                lines: 8,
            }],
            dup_total_lines: 8,
        }
    }

    #[test]
    fn render_includes_coverage_and_duplication_sections() {
        let text = sample_report().render();
        assert!(text.contains("Codecov report (local)"));
        assert!(text.contains("Coverage: 68.8% (cargo-llvm-cov)"));
        assert!(text.contains("src/codecov/mod.rs: 42.0%"));
        assert!(
            !text.contains("src/lib.rs: 81.7%"),
            "targets at or above 80% are not listed as weak"
        );
        assert!(text.contains("Duplication: 1 blocks, 8 duplicated lines"));
        assert!(text.contains("src/a.rs:1-8 <-> src/b.rs:1-8"));
    }

    #[test]
    fn render_reports_missing_tool_as_note() {
        let report = Report {
            coverage_note: Some(
                "no coverage tool found (install cargo-llvm-cov to enable)".to_string(),
            ),
            ..Report::default()
        };
        let text = report.render();
        assert!(text.contains("Coverage: skipped (no coverage tool found"));
        assert!(text.contains(&format!("blocks >= {MIN_BLOCK_LINES} lines")));
    }

    #[test]
    fn fix_prompt_lists_exact_gaps_only() {
        let prompt = sample_report().fix_prompt();
        assert!(prompt.contains("Coverage is 68.8%."));
        assert!(prompt.contains("- src/codecov/mod.rs at 42.0%"));
        assert!(
            !prompt.contains("src/lib.rs"),
            "healthy targets stay out of the fix list"
        );
        assert!(prompt.contains("- src/a.rs:1-8 and src/b.rs:1-8 (8 lines)"));
        assert!(prompt.contains("Do not refactor unrelated code."));
    }

    #[test]
    fn analyze_reports_missing_coverage_tool_without_running_anything() {
        let root = std::env::temp_dir().join(format!("codecov-analyze-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("src/lib.rs"), "fn only() {}").unwrap();
        let report = analyze(&root, None);
        assert_eq!(report.kind, ProjectKind::Rust);
        assert!(report.coverage.is_none());
        assert!(report.coverage_note.unwrap().contains("cargo-llvm-cov"));
        std::fs::remove_dir_all(&root).ok();
    }
}
