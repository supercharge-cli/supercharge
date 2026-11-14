//! Integration tests for the Supercharge validation gate.
//!
//! Lives outside the shell lib-test binary so it can be compiled without
//! rebuilding every shell unit test.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use supercharge_shell::no_mistake::{
    Finding, FindingAction, GateRequest, NoMistakeConfig, Outcome, ScriptedAdvisor, Severity,
    StaticAdvisor, StepName, StepStatus, exit_code, parse_invocation, run_with_home,
};

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

fn git_stdout(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn tempfile_dir(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join("sc-no-mistake-itests").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    base
}

#[test]
fn parse_task_and_flags() {
    let req =
        parse_invocation("--skip=lint --base-branch main -y --output-format json fix the typo")
            .unwrap();
    assert!(req.yes);
    assert_eq!(req.base_branch.as_deref(), Some("main"));
    assert_eq!(req.task, "fix the typo");
    assert!(req.skipped(StepName::Lint));
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
    request.yes = true;
    let report = run_with_home(&repo, request, cfg(), &advisor, &tmp.join("home"), false).unwrap();
    assert_eq!(report.outcome, Outcome::NeedsReview);
    assert_eq!(exit_code(report.outcome), 2);
}

#[test]
fn skip_lint_marks_passed_with_skips() {
    let tmp = tempfile_dir("skip");
    let repo = init_repo(&tmp);
    feature_commit(&repo, "hello feature\n");
    let request = parse_invocation("--skip=lint").unwrap();
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
