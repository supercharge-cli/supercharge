//! Shared fixture helpers: tiny git repos and mock agent commands so the
//! harness tests run with no network access and no model credentials.

use std::path::{Path, PathBuf};
use std::process::Command;

use supercharge_eval::dataset::Task;

pub fn run_git(repo: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git should be available")
}

/// A one-commit repo containing a single `value.txt` with the wrong content.
/// The grading contract is `test "$(cat value.txt)" = "fixed"`, so a mock agent
/// passes by writing `fixed` into that file.
pub fn init_fixture_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    run_git(&repo, &["init", "-q"]);
    run_git(&repo, &["config", "user.email", "eval@example.com"]);
    run_git(&repo, &["config", "user.name", "eval"]);
    std::fs::write(repo.join("value.txt"), "wrong\n").expect("write");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    (dir, repo)
}

pub const TEST_CMD: &str = r#"test "$(cat value.txt)" = "fixed""#;

/// Mock agent that fixes attempt 0 and breaks every other attempt, keyed off
/// the `SUPERCHARGE_EVAL_ATTEMPT` env var the runner exports.
pub const ATTEMPT_KEYED_AGENT: &str = r#"if [ "$SUPERCHARGE_EVAL_ATTEMPT" = "0" ]; then printf fixed > value.txt; else printf broken > value.txt; fi"#;

pub fn task(repo: &Path) -> Task {
    Task {
        task_id: "t1".to_string(),
        instructions: "make value.txt contain `fixed`".to_string(),
        repo_path: repo.to_path_buf(),
        base_commit: None,
        test_cmd: Some(TEST_CMD.to_string()),
        timeout_seconds: 30,
        env: Default::default(),
    }
}

pub fn eval_config(agent_script: &str, out_dir: &Path) -> supercharge_eval::EvalConfig {
    supercharge_eval::EvalConfig {
        agent: supercharge_eval::AgentCommand {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), agent_script.to_string()],
        },
        out_dir: out_dir.to_path_buf(),
        max_parallel: 2,
    }
}
