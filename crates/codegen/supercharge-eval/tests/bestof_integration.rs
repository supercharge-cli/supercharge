//! Best-of-N integration tests: fan out attempts, grade them, and verify the
//! judge selects the passing attempt.

mod support;

use supercharge_eval::bestof::{FewestChangesJudge, best_of_n};
use support::{ATTEMPT_KEYED_AGENT, eval_config, init_fixture_repo};

#[tokio::test]
async fn best_of_n_selects_the_passing_attempt() {
    let (dir, repo) = init_fixture_repo();
    let task = support::task(&repo);
    let config = eval_config(ATTEMPT_KEYED_AGENT, &dir.path().join("out"));

    // Attempt 0 fixes the task; attempts 1 and 2 write a wrong value.
    let outcome = best_of_n(&config, &task, 3, &FewestChangesJudge)
        .await
        .unwrap();

    assert_eq!(outcome.attempts.len(), 3);
    assert!(outcome.attempts[0].grade.passed());
    assert!(!outcome.attempts[1].grade.passed());
    assert_eq!(outcome.winner, Some(0), "first passing attempt wins");
    assert!(
        outcome
            .winner_diff
            .as_deref()
            .unwrap_or_default()
            .contains("fixed")
    );
    assert!(!outcome.judged_without_pass, "a verified pass exists");
}

#[tokio::test]
async fn best_of_n_without_any_pass_falls_back_to_smallest_diff() {
    let (dir, repo) = init_fixture_repo();
    let task = support::task(&repo);

    // Every attempt writes a different wrong value; no attempt passes.
    let script = r#"printf "broken-$SUPERCHARGE_EVAL_ATTEMPT" > value.txt"#;
    let config = eval_config(script, &dir.path().join("out"));
    let outcome = best_of_n(&config, &task, 2, &FewestChangesJudge)
        .await
        .unwrap();

    assert_eq!(
        outcome.winner,
        Some(0),
        "smallest change wins among failures"
    );
    assert!(
        outcome.judged_without_pass,
        "no attempt passed; judge ranked candidates"
    );
    assert!(outcome.winner_diff.is_some());
}
