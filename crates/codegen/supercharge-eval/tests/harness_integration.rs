//! End-to-end harness tests: dataset → runner → grader → metrics, using a
//! mock agent (a shell script) so no model credentials are needed.

mod support;

use supercharge_eval::cmd::eval_run;
use supercharge_eval::dataset::{Task, load_dataset};
use supercharge_eval::metrics::{Grade, RunSummary};
use supercharge_eval::{AgentCommand, EvalConfig, Runner};
use support::{ATTEMPT_KEYED_AGENT, TEST_CMD, eval_config, init_fixture_repo};

#[tokio::test]
async fn eval_run_produces_graded_summary_and_artifacts() {
    let (dir, repo) = init_fixture_repo();
    let dataset = dir.path().join("tasks.jsonl");
    std::fs::write(
        &dataset,
        format!(
            r#"{{"task_id":"t1","instructions":"fix value.txt","repo_path":"{}","test_cmd":{}}}"#,
            repo.display(),
            serde_json::to_string(TEST_CMD).unwrap(),
        ),
    )
    .unwrap();

    let config = eval_config(ATTEMPT_KEYED_AGENT, &dir.path().join("out"));
    let summary = eval_run(&config, &dataset, 2).await.unwrap();

    assert!(
        (summary.pass_at_1 - 1.0).abs() < 1e-9,
        "attempt 0 fixes the task: {summary:?}"
    );
    assert_eq!(summary.tasks, 1);
    assert_eq!(summary.grade_counts["pass"], 1);
    assert_eq!(summary.grade_counts["fail_tests"], 1);

    let run_dir = config.out_dir.join(&summary.run_id);
    for artifact in ["run.json", "grades.json", "report.md"] {
        assert!(run_dir.join(artifact).exists(), "missing {artifact}");
    }
    let report = std::fs::read_to_string(run_dir.join("report.md")).unwrap();
    assert!(
        report.contains("pass@1"),
        "report should include pass@1: {report}"
    );
}

#[tokio::test]
async fn runner_captures_patch_and_persists_run_record() {
    let (dir, repo) = init_fixture_repo();
    let task = support::task(&repo);
    let config = eval_config(ATTEMPT_KEYED_AGENT, &dir.path().join("out"));

    let run = Runner::new(config)
        .run_dataset(std::slice::from_ref(&task), 1)
        .await
        .unwrap();

    assert_eq!(run.tasks.len(), 1);
    let attempt = &run.tasks[0].attempts[0];
    assert_eq!(attempt.exit_code, 0);
    assert!(!attempt.timed_out);
    assert!(
        attempt.diff.contains("fixed"),
        "patch should contain the fix: {:?}",
        attempt.diff
    );

    let persisted = supercharge_eval::runner::load_run(
        &dir.path().join("out").join(&run.run_id).join("run.json"),
    )
    .unwrap();
    assert_eq!(persisted.run_id, run.run_id);
    assert_eq!(persisted.tasks[0].attempts[0].diff, attempt.diff);
}

#[tokio::test]
async fn grader_classifies_pass_fail_apply_and_timeout() {
    let (dir, repo) = init_fixture_repo();
    let repo_path = repo.clone();

    // Produce a real patch, then grade it: expect Pass.
    let worktree = dir.path().join("wt");
    supercharge_eval::runner::create_worktree(&repo_path, None, &worktree).unwrap();
    std::fs::write(worktree.join("value.txt"), "fixed\n").unwrap();
    let diff = supercharge_eval::runner::capture_diff(&worktree).unwrap();
    supercharge_eval::runner::remove_worktree(&repo_path, &worktree);

    let task = support::task(&repo_path);
    let record = supercharge_eval::runner::AttemptRecord {
        task_id: task.task_id.clone(),
        attempt: 0,
        exit_code: 0,
        timed_out: false,
        diff: diff.clone(),
        output_tail: String::new(),
        wall_ms: 10,
    };
    let grade = supercharge_eval::grader::Grader::default()
        .grade(&task, &record)
        .await;
    assert_eq!(grade, Grade::Pass, "fixing patch should pass");

    // A non-empty but invalid patch fails to apply.
    let garbage = supercharge_eval::runner::AttemptRecord {
        diff: "this is not a unified diff".to_string(),
        ..record.clone()
    };
    let grade = supercharge_eval::grader::Grader::default()
        .grade(&task, &garbage)
        .await;
    assert!(
        matches!(grade, Grade::FailApply { .. }),
        "garbage patch should fail apply: {grade:?}"
    );

    // An attempt that timed out grades as Timeout without running tests.
    let timed_out = supercharge_eval::runner::AttemptRecord {
        timed_out: true,
        ..record
    };
    let grade = supercharge_eval::grader::Grader::default()
        .grade(&task, &timed_out)
        .await;
    assert_eq!(grade, Grade::Timeout);
}

#[tokio::test]
async fn agent_timeout_is_killed_and_recorded() {
    let (dir, repo) = init_fixture_repo();
    let mut task = support::task(&repo);
    task.timeout_seconds = 1;

    let config = supercharge_eval::EvalConfig {
        agent: AgentCommand {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "sleep 30".to_string()],
        },
        out_dir: dir.path().join("out"),
        max_parallel: 1,
    };
    let run = Runner::new(config)
        .run_dataset(std::slice::from_ref(&task), 1)
        .await
        .unwrap();
    let attempt = &run.tasks[0].attempts[0];
    assert!(attempt.timed_out, "attempt should be recorded as timed out");
    assert!(
        attempt.diff.is_empty(),
        "no patch expected from a killed agent"
    );

    let grade = supercharge_eval::grader::Grader::default()
        .grade(&task, attempt)
        .await;
    assert_eq!(grade, Grade::Timeout);
}

#[tokio::test]
async fn dataset_loader_round_trips_fixture_file() {
    let (dir, repo) = init_fixture_repo();
    let dataset = dir.path().join("tasks.jsonl");
    std::fs::write(
        &dataset,
        format!(
            r#"{{"task_id":"t1","instructions":"fix value.txt","repo_path":"{}","test_cmd":{}}}"#,
            repo.display(),
            serde_json::to_string(TEST_CMD).unwrap(),
        ),
    )
    .unwrap();
    let tasks: Vec<Task> = load_dataset(&dataset).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task_id, "t1");
    assert_eq!(tasks[0].test_cmd.as_deref(), Some(TEST_CMD));
}

#[tokio::test]
async fn duplicate_task_ids_are_rejected() {
    let (dir, repo) = init_fixture_repo();
    let dataset = dir.path().join("tasks.jsonl");
    let line = format!(
        r#"{{"task_id":"t1","instructions":"x","repo_path":"{}"}}"#,
        repo.display()
    );
    std::fs::write(&dataset, format!("{line}\n{line}\n")).unwrap();
    let err = load_dataset(&dataset).unwrap_err().to_string();
    assert!(err.contains("duplicate task_id"), "{err}");
}

/// Keep the summary type in scope for readability of assertions above.
#[allow(dead_code)]
fn _summary_witness(_s: RunSummary) {}
