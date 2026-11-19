//! Runs a frontend project's typecheck, build and lint commands for the UI verification loop.

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::project::{FrontendProject, ProjectCommand, detect};

/// Default limit for one command.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
/// Upper bound callers may ask for.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(1800);
/// Lines of combined output kept from the end of each command.
const TAIL_LINES: usize = 80;
/// Characters of combined output kept from the end of each command.
const TAIL_CHARS: usize = 8_000;

/// One check the verification loop can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Check {
    Typecheck,
    Build,
    Lint,
}

impl Check {
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "typecheck" | "types" | "tsc" | "type-check" => Some(Self::Typecheck),
            "build" => Some(Self::Build),
            "lint" => Some(Self::Lint),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Typecheck => "typecheck",
            Self::Build => "build",
            Self::Lint => "lint",
        }
    }

    fn command(self, project: &FrontendProject) -> Option<ProjectCommand> {
        match self {
            Self::Typecheck => project.typecheck_command(),
            Self::Build => project.build_command(),
            Self::Lint => project.lint_command(),
        }
    }
}

/// What happened to one check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Passed,
    Failed,
    TimedOut,
    /// The project has no command for this check.
    Missing,
    /// The package manager or tool is not installed.
    NotFound,
    /// An earlier check failed, so this one did not run.
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub check: Check,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub output: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    pub passed: bool,
    pub project_root: String,
    pub frameworks: String,
    pub package_manager: String,
    pub results: Vec<CheckResult>,
    pub next_step: String,
}

/// Which checks to run when the caller names none: typecheck then build.
pub fn default_checks() -> Vec<Check> {
    vec![Check::Typecheck, Check::Build]
}

/// Parse the requested check names, keeping order and dropping repeats.
pub fn parse_checks(names: &[String]) -> Result<Vec<Check>, String> {
    if names.is_empty() {
        return Ok(default_checks());
    }
    let mut out = Vec::new();
    for name in names {
        let check = Check::parse(name)
            .ok_or_else(|| format!("unknown check `{name}`; use typecheck, build or lint"))?;
        if !out.contains(&check) {
            out.push(check);
        }
    }
    Ok(out)
}

/// Detect the frontend project for `workspace` and run `checks` in order, stopping at the first failure.
pub async fn verify_workspace(
    workspace: &Path,
    checks: &[Check],
    timeout: Duration,
) -> Result<VerifyReport, String> {
    let scan_dir = workspace.to_path_buf();
    let project = tokio::task::spawn_blocking(move || detect(&scan_dir))
        .await
        .map_err(|e| format!("project detection stopped: {e}"))?
        .ok_or_else(|| {
            format!(
                "no frontend project (package.json with a UI framework) was found in {} or nearby",
                workspace.display()
            )
        })?;
    Ok(verify_project(&project, checks, timeout).await)
}

/// Run `checks` for an already detected project.
pub async fn verify_project(
    project: &FrontendProject,
    checks: &[Check],
    timeout: Duration,
) -> VerifyReport {
    let mut results = Vec::with_capacity(checks.len());
    let mut failed = false;
    for &check in checks {
        let Some(command) = check.command(project) else {
            results.push(CheckResult {
                check,
                status: Status::Missing,
                command: None,
                exit_code: None,
                duration_ms: None,
                output: format!("the project has no {} script", check.name()),
            });
            continue;
        };
        if failed {
            results.push(CheckResult {
                check,
                status: Status::Skipped,
                command: Some(command.display()),
                exit_code: None,
                duration_ms: None,
                output: String::new(),
            });
            continue;
        }
        let result = run_check(check, &command, &project.root, timeout).await;
        failed = result.status != Status::Passed;
        results.push(result);
    }
    let ran_any = results.iter().any(|r| r.status == Status::Passed);
    let passed = !failed && ran_any;
    let next_step = if failed {
        "Fix the first error shown above at its cause, then call ui_verify again. Do not report done while a check fails.".to_string()
    } else if passed {
        "All checks passed. If a dev server can run, open the page with ui_browser (when available) and work the new controls before reporting done.".to_string()
    } else {
        "No check could run. Add a typecheck or build script to package.json, or run the TypeScript checker directly.".to_string()
    };
    VerifyReport {
        passed,
        project_root: project.root.display().to_string(),
        frameworks: project.frameworks_label(),
        package_manager: project.package_manager.name().to_string(),
        results,
        next_step,
    }
}

async fn run_check(
    check: Check,
    command: &ProjectCommand,
    root: &Path,
    timeout: Duration,
) -> CheckResult {
    let display = command.display();
    let started = Instant::now();
    let finish = |status: Status, exit_code: Option<i32>, output: String| CheckResult {
        check,
        status,
        command: Some(display.clone()),
        exit_code,
        duration_ms: Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)),
        output,
    };
    if which::which(&command.program).is_err() {
        return finish(
            Status::NotFound,
            None,
            format!(
                "`{}` is not on PATH; install it or run `{display}` yourself",
                command.program
            ),
        );
    }
    let mut cmd = platform_command(command);
    cmd.current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("CI", "1")
        .env("NO_COLOR", "1")
        .env("FORCE_COLOR", "0")
        .kill_on_drop(true);
    // Enrolled in the process-wide scope so an exit mid-check also stops the compiler tree.
    let (child, group) = match crate::util::spawn::global_process_scope().spawn(cmd) {
        Ok(spawned) => spawned,
        Err(e) => {
            return finish(
                Status::Failed,
                None,
                format!("could not start `{display}`: {e}"),
            );
        }
    };
    let waiting = child.wait_with_output();
    tokio::pin!(waiting);
    match tokio::time::timeout(timeout, &mut waiting).await {
        Ok(Ok(output)) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.trim().is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&stderr);
            }
            let status = if output.status.success() {
                Status::Passed
            } else {
                Status::Failed
            };
            finish(status, output.status.code(), tail(&strip_ansi(&text)))
        }
        Ok(Err(e)) => finish(
            Status::Failed,
            None,
            format!("`{display}` failed to run: {e}"),
        ),
        Err(_) => {
            // Stop the whole tree (the package manager and the compiler it started) before the leader is reaped.
            let _ = group.kill();
            finish(
                Status::TimedOut,
                None,
                format!(
                    "`{display}` did not finish within {}s and was stopped",
                    timeout.as_secs()
                ),
            )
        }
    }
}

/// Package-manager shims are `.cmd` files on Windows, which only run through `cmd /C`.
fn platform_command(command: &ProjectCommand) -> tokio::process::Command {
    if cfg!(windows) {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.arg("/C").arg(&command.program).args(&command.args);
        cmd
    } else {
        let mut cmd = tokio::process::Command::new(&command.program);
        cmd.args(&command.args);
        cmd
    }
}

/// Drop terminal colour and cursor escape sequences.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// Keep the end of long output, where compilers print the summary and the failing step.
pub fn tail(text: &str) -> String {
    let text = text.trim_end();
    let lines: Vec<&str> = text.lines().collect();
    let skipped_lines = lines.len().saturating_sub(TAIL_LINES);
    let mut kept = lines.get(skipped_lines..).unwrap_or_default().join("\n");
    let mut skipped_chars = false;
    if kept.chars().count() > TAIL_CHARS {
        let drop = kept.chars().count() - TAIL_CHARS;
        kept = kept.chars().skip(drop).collect();
        skipped_chars = true;
    }
    if skipped_lines > 0 || skipped_chars {
        format!("… (earlier output trimmed)\n{kept}")
    } else {
        kept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn checks_parse_in_order_without_repeats() {
        assert_eq!(parse_checks(&[]).unwrap(), default_checks());
        let names = ["build", "TSC", "build", "lint"].map(String::from);
        assert_eq!(
            parse_checks(&names).unwrap(),
            vec![Check::Build, Check::Typecheck, Check::Lint]
        );
        assert!(parse_checks(&["deploy".to_string()]).is_err());
    }

    #[test]
    fn ansi_sequences_are_removed() {
        let raw = "\u{1b}[31merror\u{1b}[0m TS2322 \u{1b}]8;;http://x\u{7}link\u{1b}]8;;\u{7} done";
        assert_eq!(strip_ansi(raw), "error TS2322 link done");
    }

    #[test]
    fn tail_keeps_the_end() {
        let long: String = (0..200).map(|i| format!("line {i}\n")).collect();
        let kept = tail(&long);
        assert!(kept.starts_with("… (earlier output trimmed)"));
        assert!(kept.ends_with("line 199"));
        assert!(!kept.contains("line 100\n"));
        assert_eq!(tail("ok\n"), "ok");
    }

    #[tokio::test]
    async fn no_project_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = verify_workspace(dir.path(), &default_checks(), DEFAULT_TIMEOUT)
            .await
            .unwrap_err();
        assert!(err.contains("no frontend project"), "{err}");
    }

    #[tokio::test]
    async fn missing_scripts_are_reported_not_run() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"dependencies":{"vue":"3"}}"#,
        );
        let project = detect(dir.path()).unwrap();
        let report = verify_project(&project, &[Check::Lint], DEFAULT_TIMEOUT).await;
        assert!(!report.passed);
        assert_eq!(report.results[0].status, Status::Missing);
        assert!(report.next_step.contains("No check could run"));
    }

    /// Runs real npm scripts when npm is installed; skipped otherwise.
    #[tokio::test]
    async fn failing_typecheck_stops_the_loop_and_shows_output() {
        if which::which("npm").is_err() || which::which("node").is_err() {
            eprintln!("skipping: npm/node not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"t","private":true,"dependencies":{"react":"19"},
               "scripts":{"typecheck":"node -e \"console.error('error TS2322: bad type');process.exit(2)\"",
                          "build":"node -e \"console.log('built')\""}}"#,
        );
        write(dir.path(), "package-lock.json", "{}");
        let report = verify_workspace(dir.path(), &default_checks(), DEFAULT_TIMEOUT)
            .await
            .unwrap();
        assert!(!report.passed);
        assert_eq!(report.package_manager, "npm");
        assert_eq!(report.results[0].status, Status::Failed);
        assert_eq!(report.results[0].exit_code, Some(2));
        assert!(report.results[0].output.contains("TS2322"));
        assert_eq!(report.results[1].status, Status::Skipped);

        write(
            dir.path(),
            "package.json",
            r#"{"name":"t","private":true,"dependencies":{"react":"19"},
               "scripts":{"typecheck":"node -e \"console.log('types ok')\"",
                          "build":"node -e \"console.log('built')\""}}"#,
        );
        let report = verify_workspace(dir.path(), &default_checks(), DEFAULT_TIMEOUT)
            .await
            .unwrap();
        assert!(report.passed, "{report:?}");
        assert!(report.results.iter().all(|r| r.status == Status::Passed));
        assert!(report.results[1].output.contains("built"));
    }

    #[tokio::test]
    async fn slow_checks_time_out() {
        if which::which("npm").is_err() || which::which("node").is_err() {
            eprintln!("skipping: npm/node not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"t","private":true,"dependencies":{"react":"19"},
               "scripts":{"build":"node -e \"setTimeout(()=>{},60000)\""}}"#,
        );
        let project = detect(dir.path()).unwrap();
        let report = verify_project(&project, &[Check::Build], Duration::from_secs(2)).await;
        assert_eq!(report.results[0].status, Status::TimedOut);
        assert!(!report.passed);
    }
}
