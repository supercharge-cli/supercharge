//! Coverage tool detection, low-resource command construction, and output parsing.
//!
//! Only tools already present on the machine or configured in the repo are
//! used; nothing is installed and nothing is uploaded. Parallelism is capped
//! in the constructed commands to keep CPU and memory usage low.

use std::fs;
use std::path::Path;
use std::process::Command;

use crate::detect::ProjectKind;

#[derive(Clone, Debug, PartialEq)]
pub struct CoverageTool {
    pub name: &'static str,
    pub program: &'static str,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoverageSummary {
    pub overall: Option<f64>,
    pub per_target: Vec<(String, f64)>,
}

/// Detect a coverage tool for `kind` without installing anything.
pub fn detect_coverage_tool(root: &Path, kind: ProjectKind) -> Option<CoverageTool> {
    match kind {
        ProjectKind::Rust => {
            if on_path("cargo-llvm-cov") {
                Some(CoverageTool {
                    name: "cargo-llvm-cov",
                    program: "cargo",
                    args: ["llvm-cov", "--summary-only", "--jobs", "2"]
                        .iter()
                        .map(|arg| arg.to_string())
                        .collect(),
                })
            } else if on_path("cargo-tarpaulin") {
                Some(CoverageTool {
                    name: "cargo-tarpaulin",
                    program: "cargo",
                    args: ["tarpaulin", "--skip-clean", "--jobs", "2"]
                        .iter()
                        .map(|arg| arg.to_string())
                        .collect(),
                })
            } else {
                None
            }
        }
        ProjectKind::Go => on_path("go").then(|| CoverageTool {
            name: "go test -cover",
            program: "go",
            args: ["test", "-cover", "-p", "2", "./..."]
                .iter()
                .map(|arg| arg.to_string())
                .collect(),
        }),
        ProjectKind::Python => on_path("pytest").then(|| CoverageTool {
            name: "pytest-cov",
            program: "pytest",
            args: [
                "--cov",
                "--cov-report=term-missing",
                "-q",
                "-p",
                "no:cacheprovider",
            ]
            .iter()
            .map(|arg| arg.to_string())
            .collect(),
        }),
        ProjectKind::JavaScript => has_coverage_script(root).then(|| CoverageTool {
            name: "npm run coverage",
            program: "npm",
            args: ["run", "--silent", "coverage"]
                .iter()
                .map(|arg| arg.to_string())
                .collect(),
        }),
        ProjectKind::Unknown => None,
    }
}

/// Run the tool from `root` and return stdout on success. The child inherits
/// no TTY and no network beyond what the tool itself needs for a local run.
pub fn run_coverage(root: &Path, tool: &CoverageTool) -> Option<String> {
    let output = Command::new(tool.program)
        .args(&tool.args)
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn on_path(program: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(program);
        candidate.is_file()
    })
}

fn has_coverage_script(root: &Path) -> bool {
    fs::read_to_string(root.join("package.json"))
        .map(|content| content.contains("\"coverage\""))
        .unwrap_or(false)
}

/// Parse a tool's stdout into overall and per-target coverage percentages.
/// Parsing is text-based and tolerant so partial output still reports what it can.
pub fn parse_coverage(kind: ProjectKind, stdout: &str) -> CoverageSummary {
    match kind {
        ProjectKind::Rust => parse_rust_summary(stdout),
        ProjectKind::Go => parse_go_summary(stdout),
        ProjectKind::Python => parse_total_row(stdout),
        ProjectKind::JavaScript => parse_istanbul_summary(stdout),
        ProjectKind::Unknown => CoverageSummary::default(),
    }
}

/// `cargo llvm-cov --summary-only` text rows end in a percentage; the `TOTAL`
/// row carries the overall figure.
fn parse_rust_summary(stdout: &str) -> CoverageSummary {
    let mut summary = CoverageSummary::default();
    for row in stdout.lines() {
        let Some(percent) = trailing_percent(row) else {
            continue;
        };
        let first = row.split_whitespace().next().unwrap_or_default();
        if first == "TOTAL" {
            summary.overall = Some(percent);
        } else if !first.is_empty() {
            summary.per_target.push((first.to_string(), percent));
        }
    }
    summary
}

/// `go test -cover ./...` rows look like `ok   pkg/name 0.5s coverage: 78.5% of statements`.
fn parse_go_summary(stdout: &str) -> CoverageSummary {
    let mut summary = CoverageSummary::default();
    let mut values = Vec::new();
    for row in stdout.lines() {
        let tokens: Vec<&str> = row.split_whitespace().collect();
        if tokens.len() < 4 || tokens[0] != "ok" {
            continue;
        }
        let Some(percent) = row.split("coverage:").nth(1).and_then(|rest| {
            rest.trim()
                .split('%')
                .next()
                .and_then(|value| value.trim().parse::<f64>().ok())
        }) else {
            continue;
        };
        values.push(percent);
        summary.per_target.push((tokens[1].to_string(), percent));
    }
    if !values.is_empty() {
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        summary.overall = Some((mean * 10.0).round() / 10.0);
    }
    summary
}

/// pytest-cov / coverage.py term tables: `name  Stmts Miss Branch ... NN%`,
/// with the `TOTAL` row carrying the overall figure.
fn parse_total_row(stdout: &str) -> CoverageSummary {
    let mut summary = CoverageSummary::default();
    for row in stdout.lines() {
        let Some(percent) = percent_in_row(row) else {
            continue;
        };
        let first = row.split_whitespace().next().unwrap_or_default();
        if first == "TOTAL" {
            summary.overall = Some(percent);
        } else if !first.is_empty() && !row.contains("Stmts") {
            summary.per_target.push((first.to_string(), percent));
        }
    }
    summary
}

/// istanbul / vitest text tables pipe-separate columns; `All files` is overall.
fn parse_istanbul_summary(stdout: &str) -> CoverageSummary {
    let mut summary = CoverageSummary::default();
    for row in stdout.lines() {
        if !row.contains('|') {
            continue;
        }
        let cells: Vec<&str> = row.split('|').map(|cell| cell.trim()).collect();
        let Some(percent) = cells.iter().rev().find_map(|cell| {
            let value = cell.trim_end_matches('%');
            if value.is_empty() {
                None
            } else {
                value.parse::<f64>().ok()
            }
        }) else {
            continue;
        };
        let target = cells[0].trim();
        if target == "All files" {
            summary.overall = Some(percent);
        } else if !target.is_empty() && target != "File" {
            summary.per_target.push((target.to_string(), percent));
        }
    }
    summary
}

fn trailing_percent(row: &str) -> Option<f64> {
    row.split_whitespace()
        .last()
        .and_then(|token| token.strip_suffix('%'))
        .and_then(|value| value.parse::<f64>().ok())
}

/// First percentage-looking token in the row; pytest tables place `Missing`
/// columns after `Cover`, so the percent is not always the last cell.
fn percent_in_row(row: &str) -> Option<f64> {
    row.split_whitespace()
        .find_map(|token| token.strip_suffix('%'))
        .and_then(|value| value.parse::<f64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LLVMCOV: &str = "\
    src/lib.rs      120      98     81.7%
    src/main.rs      40      12     70.0%
    TOTAL           160     110     68.8%";

    const GO: &str = "\
ok   example.com/pkg/a  0.5s  coverage: 80.0% of statements
ok   example.com/pkg/b  1.2s  coverage: 60.0% of statements
?   example.com/pkg/c  [no test files]";

    const PYTEST: &str = "\
Name                Stmts   Miss  Cover   Missing
-------------------------------------------------
src/mod_a.py           40      8    80%   12-19
src/mod_b.py           60     30    50%   1-30
TOTAL                 100     38    62%";

    const ISTANBUL: &str = "\
File         | % Stmts | % Branch | % Lines
-------------|---------|----------|--------
src/a.ts     |   85.7  |    66.6  |   85.7
src/b.ts     |   50.0  |    25.0  |   50.0
-------------|---------|----------|--------
All files    |   71.4  |    50.0  |   71.4";

    #[test]
    fn parses_llvm_cov_summary() {
        let summary = parse_coverage(ProjectKind::Rust, LLVMCOV);
        assert_eq!(summary.overall, Some(68.8));
        assert_eq!(summary.per_target.len(), 2);
        assert_eq!(summary.per_target[0], ("src/lib.rs".to_string(), 81.7));
    }

    #[test]
    fn parses_go_cover_output() {
        let summary = parse_coverage(ProjectKind::Go, GO);
        assert_eq!(summary.overall, Some(70.0));
        assert_eq!(summary.per_target.len(), 2);
        assert_eq!(
            summary.per_target[1],
            ("example.com/pkg/b".to_string(), 60.0)
        );
    }

    #[test]
    fn parses_pytest_cov_table() {
        let summary = parse_coverage(ProjectKind::Python, PYTEST);
        assert_eq!(summary.overall, Some(62.0));
        assert_eq!(summary.per_target.len(), 2);
    }

    #[test]
    fn parses_istanbul_table() {
        let summary = parse_coverage(ProjectKind::JavaScript, ISTANBUL);
        assert_eq!(summary.overall, Some(71.4));
        assert_eq!(summary.per_target.len(), 2);
        assert_eq!(summary.per_target[0], ("src/a.ts".to_string(), 85.7));
    }

    #[test]
    fn empty_output_parses_to_defaults() {
        let summary = parse_coverage(ProjectKind::Rust, "");
        assert_eq!(summary.overall, None);
        assert!(summary.per_target.is_empty());
    }
}
