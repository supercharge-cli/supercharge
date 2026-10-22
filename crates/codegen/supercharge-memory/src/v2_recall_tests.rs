use super::*;

fn scope_dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let global = temp.path().join("global");
    let workspace = temp.path().join("workspace");
    for dir in [&global, &workspace] {
        std::fs::create_dir_all(dir.join("topics")).unwrap();
        std::fs::create_dir_all(dir.join("observations/_inbox")).unwrap();
    }
    (temp, global, workspace)
}

#[test]
fn ranks_matching_note_first_and_respects_min_score() {
    let (_temp, global, workspace) = scope_dirs();
    std::fs::write(
        workspace.join("topics/build.md"),
        "---\ntitle: build\n---\n# Build\nRun cargo with CARGO_BUILD_JOBS=1 to avoid OOM on the box.\n",
    )
    .unwrap();
    std::fs::write(
        global.join("topics/style.md"),
        "# Style\nPrefer short commit messages.\n",
    )
    .unwrap();
    let backend = V2MemoryBackend::new(&global, &workspace, 0.0);
    let hits = backend.search_hits("how do I avoid OOM with cargo builds", 4, 0.0);
    assert_eq!(hits[0].path, workspace.join("topics/build.md"));
    assert_eq!(hits[0].start_line, 3, "front matter is skipped");
    assert!(hits.iter().all(|hit| hit.score < 1.0));
    assert!(backend.search_hits("cargo", 4, 0.99).is_empty());
    assert!(backend.search_hits("the and of", 4, 0.0).is_empty());
}

#[test]
fn nested_and_non_markdown_files_are_not_recalled() {
    let (_temp, global, workspace) = scope_dirs();
    std::fs::create_dir_all(workspace.join("topics/deep")).unwrap();
    std::fs::write(workspace.join("topics/deep/hidden.md"), "zebra notes\n").unwrap();
    std::fs::write(workspace.join("topics/plain.txt"), "zebra notes\n").unwrap();
    let backend = V2MemoryBackend::new(&global, &workspace, 0.0);
    assert!(backend.search_hits("zebra", 4, 0.0).is_empty());
}

#[test]
fn get_reads_only_notes_inside_scopes() {
    let (temp, global, workspace) = scope_dirs();
    std::fs::write(workspace.join("topics/a.md"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(workspace.join("memory_state.sqlite"), "db").unwrap();
    std::fs::write(temp.path().join("outside.md"), "secret").unwrap();
    let backend = V2MemoryBackend::new(&global, &workspace, 0.0);
    assert_eq!(backend.get("topics/a.md", Some(2), Some(1)).unwrap(), "two");
    let absolute = workspace.join("topics/a.md");
    assert!(backend.get(absolute.to_str().unwrap(), None, None).is_ok());
    assert!(backend.get("memory_state.sqlite", None, None).is_err());
    assert!(backend.get("../outside.md", None, None).is_err());
    assert!(backend.get("topics/../../outside.md", None, None).is_err());
}

#[test]
fn long_notes_split_into_chunks() {
    let (_temp, global, workspace) = scope_dirs();
    let mut body = String::new();
    for i in 0..20 {
        body.push_str(&format!("Paragraph {i} about general things and more words here.\n\n"));
    }
    body.push_str("Final paragraph mentions the flamingo deployment.\n");
    std::fs::write(workspace.join("topics/long.md"), body).unwrap();
    let backend = V2MemoryBackend::new(&global, &workspace, 0.0);
    let hits = backend.search_hits("flamingo", 4, 0.0);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].start_line > 0);
    assert!(hits[0].text.len() < 1200);
}
