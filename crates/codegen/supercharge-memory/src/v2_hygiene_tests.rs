use super::*;

const DAY: i64 = 86_400;
const NOW: i64 = 1_800_000_000;

fn scope() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("memory-v2/workspaces/ws");
    crate::v2::ensure_scope_initialized(&temp.path().join("memory-v2"), &dir, V2MemoryScope::Workspace)
        .unwrap();
    (temp, dir)
}

fn topic(dir: &Path, name: &str, body: &str, created: i64, used: Option<i64>) {
    std::fs::write(dir.join("topics").join(name), body).unwrap();
    let mut ledger = V2UsageLedger::default();
    let relative = format!("topics/{name}");
    ledger.ensure_created(&relative, created);
    if let Some(used) = used {
        ledger.mark_used(&relative, used);
    }
    ledger.save_merged(dir, &[]).unwrap();
}

#[test]
fn archives_topics_unused_past_stale_days() {
    let (_temp, dir) = scope();
    topic(&dir, "old.md", "# Old\nLegacy deploy steps for the staging cluster\n", NOW - 200 * DAY, Some(NOW - 91 * DAY));
    topic(&dir, "used.md", "# Used\nRun the formatter before committing changes\n", NOW - 200 * DAY, Some(NOW - 10 * DAY));
    topic(&dir, "new.md", "# New\nPrefer small focused pull requests always\n", NOW - DAY, None);
    let report = run_topic_hygiene(&dir, V2MemoryScope::Workspace, NOW, V2HygienePolicy::default()).unwrap();
    assert_eq!(report.archived_stale, vec![dir.join("topics/old.md")]);
    assert!(!dir.join("topics/old.md").exists());
    assert!(dir.join(format!("archive/topics/old.{NOW}.md")).exists());
    assert!(dir.join("topics/used.md").exists() && dir.join("topics/new.md").exists());
    assert!(V2UsageLedger::load(&dir).get("topics/old.md").is_none());
    let manifest = std::fs::read_to_string(dir.join("MEMORY.md")).unwrap();
    assert!(!manifest.contains("old.md"));

    // The same store a day earlier keeps everything.
    let (_temp, dir) = scope();
    topic(&dir, "old.md", "# Old\nLegacy deploy steps\n", NOW - 200 * DAY, Some(NOW - 89 * DAY));
    let report = run_topic_hygiene(&dir, V2MemoryScope::Workspace, NOW, V2HygienePolicy::default()).unwrap();
    assert!(report.is_empty());
}

#[test]
fn stale_days_zero_disables_archiving() {
    let (_temp, dir) = scope();
    topic(&dir, "old.md", "# Old\nAncient\n", 0, None);
    let policy = V2HygienePolicy { stale_days: 0, ..Default::default() };
    assert!(run_topic_hygiene(&dir, V2MemoryScope::Workspace, NOW, policy).unwrap().is_empty());
    assert!(dir.join("topics/old.md").exists());
}

#[test]
fn merges_near_duplicates_into_most_recently_used() {
    let (_temp, dir) = scope();
    topic(
        &dir,
        "build.md",
        "# Build\n- Use CARGO_BUILD_JOBS=1 on the small box to avoid running out of memory\n",
        NOW - 30 * DAY,
        Some(NOW - DAY),
    );
    topic(
        &dir,
        "builds.md",
        "# Builds\n- Use CARGO_BUILD_JOBS=1 on the small box to avoid running out of memory\n- Enable the swapfile\n",
        NOW - 40 * DAY,
        Some(NOW - 5 * DAY),
    );
    topic(&dir, "style.md", "# Style\n- Prefer descriptive variable names in all new code\n", NOW - DAY, None);
    let report = run_topic_hygiene(&dir, V2MemoryScope::Workspace, NOW, V2HygienePolicy::default()).unwrap();
    assert_eq!(report.merged, vec![(dir.join("topics/build.md"), dir.join("topics/builds.md"))]);
    let kept = std::fs::read_to_string(dir.join("topics/build.md")).unwrap();
    assert!(kept.contains("## Merged from builds.md"), "{kept}");
    assert!(kept.contains("Enable the swapfile"));
    assert_eq!(kept.matches("CARGO_BUILD_JOBS").count(), 1, "{kept}");
    assert!(!dir.join("topics/builds.md").exists());
    assert!(dir.join("topics/style.md").exists());
    let usage = V2UsageLedger::load(&dir).get("topics/build.md").unwrap();
    assert_eq!(usage.created_at, NOW - 40 * DAY);
    assert_eq!(usage.last_used_at, Some(NOW - DAY));
}

#[test]
fn distinct_topics_are_left_alone() {
    let (_temp, dir) = scope();
    topic(&dir, "a.md", "# A\nDatabase migrations run with the migrate binary\n", NOW, None);
    topic(&dir, "b.md", "# B\nFrontend tests use the browser runner\n", NOW, None);
    assert!(run_topic_hygiene(&dir, V2MemoryScope::Workspace, NOW, V2HygienePolicy::default()).unwrap().is_empty());
}
