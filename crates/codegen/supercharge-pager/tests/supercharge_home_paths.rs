//! `SUPERCHARGE_HOME` override tests in an isolated binary so `supercharge_home()`'s process-wide `OnceLock` initializes from the overridden env var.

use std::path::PathBuf;

#[test]
#[serial_test::serial(SUPERCHARGE_HOME)]
fn supercharge_home_override_path_helpers() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let supercharge_home = tmp.path().to_path_buf();
    unsafe {
        std::env::set_var("SUPERCHARGE_HOME", &supercharge_home);
    }

    assert_eq!(
        supercharge_pager::util::pager_toml_path(),
        supercharge_home.join("pager.toml")
    );
    assert_eq!(
        supercharge_pager::util::display_supercharge_home_prefix(),
        "$SUPERCHARGE_HOME"
    );
    assert_eq!(
        supercharge_pager::util::display_user_grok_path("config.toml"),
        "$SUPERCHARGE_HOME/config.toml"
    );

    let memory_path = supercharge_home.join("memory/MEMORY.md");
    assert_eq!(
        supercharge_pager::util::abbreviate_path(&memory_path.display().to_string()),
        "$SUPERCHARGE_HOME/memory/MEMORY.md"
    );

    // The copy toast abbreviates paths the same way, so a custom $SUPERCHARGE_HOME outside $HOME still shows the short form
    assert_eq!(
        supercharge_pager::clipboard::display_copy_path(&supercharge_home.join("last-copy.txt")),
        "$SUPERCHARGE_HOME/last-copy.txt"
    );

    assert!(supercharge_pager::util::is_under_user_supercharge_home(
        &memory_path
    ));
    assert!(!supercharge_pager::util::is_under_user_supercharge_home(
        PathBuf::from("/tmp/other").as_path()
    ));
}

/// Isolated because `supercharge_home()`'s `OnceLock` is already initialized by the time the shared lib-test binary reaches a case like this.
#[test]
#[serial_test::serial(SUPERCHARGE_HOME)]
fn disk_usage_run_creates_no_supercharge_home() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ghost = tmp.path().join("ghost-home");
    unsafe {
        std::env::set_var("SUPERCHARGE_HOME", &ghost);
    }

    for json in [false, true] {
        supercharge_pager::disk_usage_cmd::run(supercharge_pager::disk_usage_cmd::DiskUsageArgs {
            json,
        })
        .expect("a missing home is not an error");
        assert!(
            !ghost.exists(),
            "grok du must not create the home it reports on (json={json})"
        );
    }
}
