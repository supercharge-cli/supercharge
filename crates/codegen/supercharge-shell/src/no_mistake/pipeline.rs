//! Pipeline runner for the Supercharge validation gate.

use std::fmt;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::no_mistake::advisor::{Advisor, FixContext, ReviewContext};
use crate::no_mistake::args::GateRequest;
use crate::no_mistake::config::NoMistakeConfig;
use crate::no_mistake::findings::{Finding, FindingAction, Severity};
use crate::no_mistake::gitops::{self, RepoInfo};
use crate::no_mistake::report::{GateReport, Outcome, StepName, StepReport, StepStatus};

/// A recoverable or user-facing gate error (never a panic).
#[derive(Debug)]
pub enum GateError {
    User(String),
    Io {
        context: String,
        source: std::io::Error,
    },
}

impl GateError {
    pub fn user(message: impl Into<String>) -> Self {
        Self::User(message.into())
    }

    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

impl fmt::Display for GateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User(message) => f.write_str(message),
            Self::Io { context, source } => {
                write!(f, "Could not {context}: {source}")
            }
        }
    }
}

impl std::error::Error for GateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::User(_) => None,
        }
    }
}

struct RunState<'a> {
    request: &'a GateRequest,
    config: &'a NoMistakeConfig,
    advisor: &'a dyn Advisor,
    home: PathBuf,
    repo: RepoInfo,
    intent: String,
    worktree: Option<PathBuf>,
    gate_branch: Option<String>,
    steps: Vec<StepReport>,
    skipped_explicit: bool,
    needs_review: bool,
    failed: bool,
    cancelled: bool,
    keep_worktree: bool,
    interactive: bool,
}

impl RunState<'_> {
    fn should_skip(&self, step: StepName) -> bool {
        if self.request.skipped(step) {
            return true;
        }
        if step == StepName::Document && !self.request.wants_publish() {
            // Documentation checks land with the publish work. Local-only runs skip them.
            // A later slice enables the step even without publish.
            return true;
        }
        if step.is_publish() && !self.request.wants_publish() {
            return true;
        }
        false
    }
}

/// Run the validation gate starting from `cwd`.
pub fn run(
    cwd: &Path,
    request: GateRequest,
    config: NoMistakeConfig,
    advisor: &dyn Advisor,
) -> Result<GateReport, GateError> {
    run_with_home(
        cwd,
        request,
        config,
        advisor,
        &supercharge_dirs::supercharge_home(),
        atty_stderr(),
    )
}

/// Same as [`run`] with an explicit worktree home (used by tests).
pub fn run_with_home(
    cwd: &Path,
    request: GateRequest,
    config: NoMistakeConfig,
    advisor: &dyn Advisor,
    home: &Path,
    interactive: bool,
) -> Result<GateReport, GateError> {
    let intent = request
        .intent_text()
        .map_err(|e| GateError::user(e.message().to_string()))?;

    let preferred_base = request
        .base_branch
        .as_deref()
        .or(config.base_branch.as_deref());
    let repo = gitops::inspect_repo(cwd, preferred_base)?;

    let mut state = RunState {
        request: &request,
        config: &config,
        advisor,
        home: home.to_path_buf(),
        repo,
        intent,
        worktree: None,
        gate_branch: None,
        steps: Vec::new(),
        skipped_explicit: !request.skip.is_empty(),
        needs_review: false,
        failed: false,
        cancelled: false,
        keep_worktree: false,
        interactive,
    };

    for step in StepName::ALL {
        if state.failed || state.cancelled || state.needs_review {
            state.steps.push(
                StepReport::new(step, StepStatus::Pending)
                    .with_detail("Not run because an earlier step stopped the gate."),
            );
            continue;
        }
        if state.should_skip(step) {
            if state.request.skipped(step) {
                state.skipped_explicit = true;
            }
            let reason = skip_reason(step, state.request.wants_publish());
            state
                .steps
                .push(StepReport::new(step, StepStatus::Skipped).with_detail(reason));
            continue;
        }
        match run_step(&mut state, step) {
            Ok(report) => {
                match report.status {
                    StepStatus::Failed => state.failed = true,
                    StepStatus::Cancelled => state.cancelled = true,
                    StepStatus::NeedsReview => state.needs_review = true,
                    _ => {}
                }
                state.steps.push(report);
            }
            Err(GateError::User(message)) => {
                state.failed = true;
                state.keep_worktree = true;
                state
                    .steps
                    .push(StepReport::new(step, StepStatus::Failed).with_detail(message));
            }
            Err(err) => {
                cleanup(&state);
                return Err(err);
            }
        }
    }

    let outcome = if state.cancelled {
        Outcome::Cancelled
    } else if state.failed {
        Outcome::Failed
    } else if state.needs_review {
        Outcome::NeedsReview
    } else if state.skipped_explicit {
        Outcome::PassedWithSkips
    } else {
        Outcome::Passed
    };

    if !state.keep_worktree && outcome.is_success() {
        cleanup(&state);
    }

    let message = match outcome {
        Outcome::Passed | Outcome::PassedWithSkips if !request.wants_publish() => {
            Some("Local checks finished. Push, pull request, and CI were not run. Pass --publish to include them when those steps are available.".to_string())
        }
        Outcome::NeedsReview => Some(
            "The gate paused for a human decision. Re-run after addressing the findings, or pass --yes only for auto-fixable issues. Ask-user findings never auto-resolve."
                .to_string(),
        ),
        _ => None,
    };

    Ok(GateReport {
        outcome,
        intent: state.intent,
        branch: Some(state.repo.branch.clone()),
        base_branch: Some(state.repo.default_branch.clone()),
        worktree: state.worktree.as_ref().map(|p| p.display().to_string()),
        steps: state.steps,
        message,
    })
}

fn skip_reason(step: StepName, wants_publish: bool) -> String {
    if step.is_publish() && !wants_publish {
        return "Skipped because this run is local-only (no --publish).".to_string();
    }
    if step == StepName::Document && !wants_publish {
        return "Documentation checks run with the publish steps. Pass --publish or skip explicitly.".to_string();
    }
    format!("{} skipped by request.", step.as_str())
}

fn run_step(state: &mut RunState<'_>, step: StepName) -> Result<StepReport, GateError> {
    match step {
        StepName::Intent => step_intent(state),
        StepName::Isolate => step_isolate(state),
        StepName::Rebase => step_rebase(state),
        StepName::Review => step_review(state),
        StepName::Test => step_command(
            state,
            StepName::Test,
            state.config.commands.test.clone(),
            gitops::infer_test_command,
            "No test command is configured and none could be inferred from the repository. Set [no_mistake.commands].test or pass --skip=test.",
        ),
        StepName::Document => Ok(StepReport::new(StepName::Document, StepStatus::Skipped)
            .with_detail("Documentation checks are not run in a local-only gate.")),
        StepName::Lint => step_command(
            state,
            StepName::Lint,
            state.config.commands.lint.clone(),
            gitops::infer_lint_command,
            "No lint command is configured and none could be inferred from the repository. Set [no_mistake.commands].lint or pass --skip=lint.",
        ),
        StepName::Push | StepName::Pr | StepName::Ci => {
            Ok(StepReport::new(step, StepStatus::Skipped)
                .with_detail("Publish steps are not enabled on this run."))
        }
    }
}

fn step_intent(state: &mut RunState<'_>) -> Result<StepReport, GateError> {
    let info = &state.repo;
    if gitops::is_default_branch(&info.branch, &info.default_branch) {
        return Ok(StepReport::new(StepName::Intent, StepStatus::Failed).with_detail(
            format!(
                "You are on the default branch '{}'. Create a feature branch and commit the work before running /no-mistake.",
                info.branch
            ),
        ));
    }
    if gitops::has_uncommitted_changes(&info.root)? {
        return Ok(StepReport::new(StepName::Intent, StepStatus::Failed).with_detail(
            "There are uncommitted changes in the working tree. Commit only the work you want gated, then run /no-mistake again. Unrelated dirty files were left untouched.",
        ));
    }
    if state.intent.is_empty() {
        let commits =
            gitops::commits_ahead(&info.root, &info.default_branch, "HEAD").unwrap_or_default();
        if let Some(first) = commits.first() {
            state.intent = first.clone();
        } else {
            return Ok(StepReport::new(StepName::Intent, StepStatus::Failed).with_detail(
                "No intent was provided and no commits were found ahead of the base branch. Pass a task: /no-mistake <task>.",
            ));
        }
    }
    Ok(StepReport::new(StepName::Intent, StepStatus::Passed).with_detail(state.intent.clone()))
}

fn step_isolate(state: &mut RunState<'_>) -> Result<StepReport, GateError> {
    let (path, gate_branch, _) =
        gitops::create_isolated_worktree(&state.repo.root, &state.home, &state.repo.branch)?;
    let display = path.display().to_string();
    state.worktree = Some(path);
    state.gate_branch = Some(gate_branch.clone());
    Ok(StepReport::new(StepName::Isolate, StepStatus::Passed)
        .with_detail(format!("Isolated worktree on {gate_branch} at {display}")))
}

fn step_rebase(state: &mut RunState<'_>) -> Result<StepReport, GateError> {
    let Some(worktree) = state.worktree.as_ref() else {
        return Ok(StepReport::new(StepName::Rebase, StepStatus::Failed)
            .with_detail("No isolated worktree is available to rebase."));
    };
    gitops::rebase_onto(worktree, &state.repo.default_branch)?;
    let diff = gitops::unified_diff(worktree, &state.repo.default_branch)?;
    if diff.trim().is_empty() {
        return Ok(StepReport::new(StepName::Rebase, StepStatus::Passed).with_detail(
            "Rebase completed and produced an empty diff against the base branch. Later local checks were not needed.",
        ));
    }
    Ok(StepReport::new(StepName::Rebase, StepStatus::Passed)
        .with_detail(format!("Rebased onto {}", state.repo.default_branch)))
}

fn step_review(state: &mut RunState<'_>) -> Result<StepReport, GateError> {
    let Some(worktree) = state.worktree.as_ref() else {
        return Ok(StepReport::new(StepName::Review, StepStatus::Failed)
            .with_detail("No isolated worktree is available to review."));
    };
    let diff = gitops::unified_diff(worktree, &state.repo.default_branch)?;
    if diff.trim().is_empty() {
        return Ok(StepReport::new(StepName::Review, StepStatus::Skipped)
            .with_detail("No diff against the base branch."));
    }
    let ctx = ReviewContext {
        intent: &state.intent,
        branch: &state.repo.branch,
        base_branch: &state.repo.default_branch,
        worktree,
        diff: &diff,
    };
    let mut findings = state.advisor.review(&ctx)?;
    let limit = state.config.auto_fix_limit(StepName::Review);
    let mut attempts = 0u32;
    while attempts < limit && findings.iter().any(|f| f.action == FindingAction::AutoFix) {
        attempts = attempts.saturating_add(1);
        let auto: Vec<Finding> = findings
            .iter()
            .filter(|f| f.action == FindingAction::AutoFix)
            .cloned()
            .collect();
        if auto.is_empty() {
            break;
        }
        let changed = state.advisor.apply_fixes(&FixContext {
            intent: &state.intent,
            worktree,
            findings: &auto,
        })?;
        if changed {
            let _ = gitops::commit_all(
                worktree,
                &format!("fix: Supercharge no-mistake review auto-fix ({attempts})"),
            )?;
        }
        let diff = gitops::unified_diff(worktree, &state.repo.default_branch)?;
        findings = state.advisor.review(&ReviewContext {
            intent: &state.intent,
            branch: &state.repo.branch,
            base_branch: &state.repo.default_branch,
            worktree,
            diff: &diff,
        })?;
    }
    decide_findings(state, StepName::Review, findings)
}

fn step_command(
    state: &mut RunState<'_>,
    step: StepName,
    configured: Option<String>,
    infer: fn(&Path) -> Option<String>,
    missing_message: &str,
) -> Result<StepReport, GateError> {
    let Some(worktree) = state.worktree.as_ref() else {
        return Ok(StepReport::new(step, StepStatus::Failed)
            .with_detail("No isolated worktree is available."));
    };
    let command = match configured.or_else(|| infer(worktree)) {
        Some(cmd) => cmd,
        None => {
            return Ok(
                StepReport::new(step, StepStatus::Failed).with_detail(missing_message.to_string())
            );
        }
    };
    let limit = state.config.auto_fix_limit(step);
    let mut attempts = 0u32;
    loop {
        let result = gitops::run_shell_command(worktree, &command)?;
        if result.success {
            return Ok(
                StepReport::new(step, StepStatus::Passed).with_detail(format!("Ran `{command}`"))
            );
        }
        let output = truncate(&result.combined(), 4_000);
        let finding = Finding::new(
            format!("{}-failed", step.as_str()),
            Severity::Error,
            if attempts < limit {
                FindingAction::AutoFix
            } else {
                FindingAction::AskUser
            },
            format!("`{command}` failed:\n{output}"),
        );
        if attempts < limit {
            attempts = attempts.saturating_add(1);
            let changed = state.advisor.apply_fixes(&FixContext {
                intent: &state.intent,
                worktree,
                findings: std::slice::from_ref(&finding),
            })?;
            if changed {
                let _ = gitops::commit_all(
                    worktree,
                    &format!(
                        "fix: Supercharge no-mistake {} auto-fix ({attempts})",
                        step.as_str()
                    ),
                )?;
                continue;
            }
        }
        let mut report = decide_findings(state, step, vec![finding])?;
        if report.detail.is_none() {
            report.detail = Some(format!("`{command}` failed"));
        }
        return Ok(report);
    }
}

fn decide_findings(
    state: &mut RunState<'_>,
    step: StepName,
    findings: Vec<Finding>,
) -> Result<StepReport, GateError> {
    if findings.is_empty() {
        return Ok(StepReport::new(step, StepStatus::Passed));
    }
    let ask = findings
        .iter()
        .any(|f| f.action == FindingAction::AskUser || f.severity.is_blocking());
    let auto_remaining = findings.iter().any(|f| f.action == FindingAction::AutoFix);
    if !ask && !auto_remaining {
        let mut report = StepReport::new(step, StepStatus::Passed)
            .with_detail("Completed with non-blocking findings.");
        report.findings = findings;
        return Ok(report);
    }
    if ask && !(state.request.yes && state.interactive) {
        // Fail closed: --yes never auto-resolves ask-user findings, and
        // a non-interactive run cannot prompt.
        state.keep_worktree = true;
        let mut report = StepReport::new(step, StepStatus::NeedsReview)
            .with_detail("Paused for a human decision. Ask-user findings are never auto-resolved.");
        report.findings = findings;
        return Ok(report);
    }
    if auto_remaining && !state.request.yes {
        state.keep_worktree = true;
        let mut report = StepReport::new(step, StepStatus::NeedsReview).with_detail(
            "Auto-fixable findings remain. Re-run with --yes to apply them, or fix them yourself.",
        );
        report.findings = findings;
        return Ok(report);
    }
    // --yes on an interactive session with only auto-fix leftovers already
    // exhausted the limit; treat remaining blocking issues as needs-review.
    state.keep_worktree = true;
    let mut report = StepReport::new(step, StepStatus::NeedsReview)
        .with_detail("Findings remain after the auto-fix limit was reached.");
    report.findings = findings;
    Ok(report)
}

fn cleanup(state: &RunState<'_>) {
    if let (Some(path), Some(branch)) = (&state.worktree, &state.gate_branch) {
        gitops::remove_worktree(&state.repo.root, path, branch);
    }
}

fn atty_stderr() -> bool {
    // Fail closed when we cannot tell. A TTY on stderr is required for prompts.
    std::io::stderr().is_terminal()
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        text.to_string()
    } else {
        let end = text.floor_char_boundary(max);
        let mut s = text.get(..end).unwrap_or(text).to_string();
        s.push('…');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::no_mistake::GateRequest;
    use crate::no_mistake::advisor::{ScriptedAdvisor, StaticAdvisor};
    use crate::no_mistake::findings::{Finding, FindingAction, Severity};
    use std::fs;
    use std::process::Command;

    fn git(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "Supercharge")
            .env("GIT_AUTHOR_EMAIL", "dev@supercharge.local")
            .env("GIT_COMMITTER_NAME", "Supercharge")
            .env("GIT_COMMITTER_EMAIL", "dev@supercharge.local")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo(tmp: &Path) -> PathBuf {
        let repo = tmp.join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.name", "Supercharge"]);
        git(&repo, &["config", "user.email", "dev@supercharge.local"]);
        fs::write(repo.join("README.md"), "hello\n").unwrap();
        // A passing test and lint command used by the gate.
        fs::write(repo.join("check.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "initial"]);
        repo
    }

    fn feature_commit(repo: &Path, body: &str) {
        git(repo, &["checkout", "-b", "feat/demo"]);
        fs::write(repo.join("README.md"), body).unwrap();
        git(repo, &["add", "README.md"]);
        git(repo, &["commit", "-m", "feat: update readme"]);
    }

    fn cfg() -> NoMistakeConfig {
        let mut cfg = NoMistakeConfig::default();
        cfg.commands.test = Some("sh check.sh".into());
        cfg.commands.lint = Some("sh check.sh".into());
        cfg
    }

    #[test]
    fn rejects_default_branch() {
        let tmp = tempfile_dir("default-branch");
        let repo = init_repo(&tmp);
        let report = run_with_home(
            &repo,
            GateRequest::default(),
            cfg(),
            &StaticAdvisor,
            &tmp.join("home"),
            false,
        )
        .unwrap();
        assert_eq!(report.outcome, Outcome::Failed);
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.name == StepName::Intent && s.status == StepStatus::Failed)
        );
    }

    #[test]
    fn rejects_dirty_tree() {
        let tmp = tempfile_dir("dirty");
        let repo = init_repo(&tmp);
        feature_commit(&repo, "hello feature\n");
        fs::write(repo.join("dirty.txt"), "nope\n").unwrap();
        let report = run_with_home(
            &repo,
            GateRequest::default(),
            cfg(),
            &StaticAdvisor,
            &tmp.join("home"),
            false,
        )
        .unwrap();
        assert_eq!(report.outcome, Outcome::Failed);
        let intent = report
            .steps
            .iter()
            .find(|s| s.name == StepName::Intent)
            .unwrap();
        assert_eq!(intent.status, StepStatus::Failed);
        assert!(
            intent
                .detail
                .as_deref()
                .unwrap_or("")
                .contains("uncommitted")
        );
        // Original dirty file is still there.
        assert!(repo.join("dirty.txt").is_file());
    }

    #[test]
    fn local_gate_passes_on_clean_feature_branch() {
        let tmp = tempfile_dir("pass");
        let repo = init_repo(&tmp);
        feature_commit(&repo, "hello feature\n");
        let report = run_with_home(
            &repo,
            GateRequest::default(),
            cfg(),
            &StaticAdvisor,
            &tmp.join("home"),
            false,
        )
        .unwrap();
        assert_eq!(report.outcome, Outcome::Passed, "{}", report.render_plain());
        assert_eq!(report.intent, "feat: update readme");
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.name == StepName::Review && s.status == StepStatus::Passed)
        );
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.name == StepName::Test && s.status == StepStatus::Passed)
        );
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.name == StepName::Lint && s.status == StepStatus::Passed)
        );
        assert!(
            report
                .steps
                .iter()
                .all(|s| !s.name.is_publish() || s.status == StepStatus::Skipped)
        );
        // User checkout still on the same commit and branch, worktree cleaned up on success.
        let branch = git_stdout(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]);
        assert_eq!(branch, "feat/demo");
    }

    #[test]
    fn ask_user_findings_fail_closed_without_tty() {
        let tmp = tempfile_dir("ask");
        let repo = init_repo(&tmp);
        feature_commit(&repo, "hello feature\n");
        let advisor = ScriptedAdvisor {
            findings: vec![Finding::new(
                "r1",
                Severity::Error,
                FindingAction::AskUser,
                "Needs a human look.",
            )],
            apply: false,
        };
        let mut request = GateRequest::default();
        request.yes = true; // must still fail closed for ask-user / non-interactive
        let report =
            run_with_home(&repo, request, cfg(), &advisor, &tmp.join("home"), false).unwrap();
        assert_eq!(report.outcome, Outcome::NeedsReview);
        assert_eq!(exit_code_for(&report), 2);
    }

    #[test]
    fn skip_lint_marks_passed_with_skips() {
        let tmp = tempfile_dir("skip");
        let repo = init_repo(&tmp);
        feature_commit(&repo, "hello feature\n");
        let request = crate::no_mistake::parse_invocation("--skip=lint").unwrap();
        let report = run_with_home(
            &repo,
            request,
            cfg(),
            &StaticAdvisor,
            &tmp.join("home"),
            false,
        )
        .unwrap();
        assert_eq!(report.outcome, Outcome::PassedWithSkips);
        let lint = report
            .steps
            .iter()
            .find(|s| s.name == StepName::Lint)
            .unwrap();
        assert_eq!(lint.status, StepStatus::Skipped);
    }

    #[test]
    fn failing_test_command_reports_failure_output() {
        let tmp = tempfile_dir("fail-test");
        let repo = init_repo(&tmp);
        feature_commit(&repo, "hello feature\n");
        let mut cfg = cfg();
        cfg.commands.test = Some("sh -c 'echo boom >&2; exit 1'".into());
        cfg.auto_fix.test = 0;
        let report = run_with_home(
            &repo,
            GateRequest::default(),
            cfg,
            &StaticAdvisor,
            &tmp.join("home"),
            false,
        )
        .unwrap();
        assert_eq!(report.outcome, Outcome::NeedsReview);
        let test = report
            .steps
            .iter()
            .find(|s| s.name == StepName::Test)
            .unwrap();
        assert_eq!(test.status, StepStatus::NeedsReview);
        assert!(test.findings.iter().any(|f| f.description.contains("boom")));
    }

    #[test]
    fn original_tree_is_untouched_after_isolate() {
        let tmp = tempfile_dir("untouched");
        let repo = init_repo(&tmp);
        feature_commit(&repo, "hello feature\n");
        let head_before = git_stdout(&repo, &["rev-parse", "HEAD"]);
        let _ = run_with_home(
            &repo,
            GateRequest::default(),
            cfg(),
            &StaticAdvisor,
            &tmp.join("home"),
            false,
        )
        .unwrap();
        let head_after = git_stdout(&repo, &["rev-parse", "HEAD"]);
        assert_eq!(head_before, head_after);
        let status = git_stdout(&repo, &["status", "--porcelain"]);
        assert!(status.is_empty(), "{status}");
    }

    fn exit_code_for(report: &GateReport) -> i32 {
        crate::no_mistake::exit_code(report.outcome)
    }

    fn git_stdout(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn tempfile_dir(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join("sc-no-mistake-tests").join(name);
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }
}
