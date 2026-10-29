use super::*;

#[test]
fn creates_missing_parent_and_leaves_no_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a").join("config.toml");
    write_scoped(&path, "x = 1\n", ImportScope::Project).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "x = 1\n");
    let names: Vec<_> = std::fs::read_dir(dir.path().join("a"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("config.toml")]);
}

#[cfg(unix)]
#[test]
fn global_write_follows_a_symlink_chain() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("dotfiles").join("config.toml");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::write(&real, "old = true\n").unwrap();
    let hop = dir.path().join("hop.toml");
    std::os::unix::fs::symlink(&real, &hop).unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let link = home.join("config.toml");
    // Relative link target, resolved against the link's own directory.
    std::os::unix::fs::symlink("../hop.toml", &link).unwrap();

    write_scoped(&link, "new = true\n", ImportScope::Global).unwrap();

    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        std::fs::symlink_metadata(&hop)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "new = true\n");
}

#[cfg(unix)]
#[test]
fn global_write_through_a_dangling_symlink_creates_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("store").join("config.toml");
    let link = dir.path().join("config.toml");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    write_scoped(&link, "a = 1\n", ImportScope::Global).unwrap();

    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "a = 1\n");
}

#[cfg(unix)]
#[test]
fn project_write_replaces_a_symlink_instead_of_writing_through_it() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside.toml");
    std::fs::write(&outside, "keep = true\n").unwrap();
    let project = dir.path().join("repo").join(".supercharge");
    std::fs::create_dir_all(&project).unwrap();
    let link = project.join("config.toml");
    std::os::unix::fs::symlink(&outside, &link).unwrap();

    write_scoped(&link, "new = true\n", ImportScope::Project).unwrap();

    assert!(
        !std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read_to_string(&link).unwrap(), "new = true\n");
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep = true\n");
}

#[cfg(unix)]
#[test]
fn global_write_rejects_a_symlink_loop() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.toml");
    let b = dir.path().join("b.toml");
    std::os::unix::fs::symlink(&b, &a).unwrap();
    std::os::unix::fs::symlink(&a, &b).unwrap();

    let error = write_scoped(&a, "x = 1\n", ImportScope::Global).unwrap_err();
    assert!(error.to_string().contains("symbolic links"), "{error}");
    assert!(
        std::fs::symlink_metadata(&a)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
