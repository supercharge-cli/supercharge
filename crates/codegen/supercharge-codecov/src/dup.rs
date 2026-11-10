//! Self-contained duplicate-code scanner.
//!
//! Each file is normalized (trimmed, blank and comment-only lines dropped) and
//! hashed in 6-line sliding windows. A window is "matched" when its hash also
//! occurs in another file (or a disjoint range of the same file); consecutive
//! matched windows chain into maximal segments, and segments from different
//! files (or disjoint ranges) pair into reported blocks. Files stream one at a
//! time and memory is one hash plus one location per window, so the scan is
//! cheap on CPU and RAM.

use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Minimum normalized lines for a duplicate block to be reported.
pub const MIN_BLOCK_LINES: usize = 6;

const WINDOW: usize = MIN_BLOCK_LINES;

const SOURCE_EXTS: &[&str] = &[
    "rs", "go", "py", "js", "jsx", "ts", "tsx", "java", "c", "h", "cpp", "hpp", "rb", "php", "sh",
];

const IGNORED_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    "out",
    "vendor",
    "coverage",
    "venv",
    ".venv",
    "__pycache__",
];

/// Cap total segments so pathological boilerplate stays linear.
const MAX_SEGMENTS: usize = 256;

#[derive(Clone, Debug, PartialEq)]
pub struct DupLocation {
    pub file: String,
    /// 1-based inclusive, in original file line numbers.
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DupBlock {
    pub locations: Vec<DupLocation>,
    pub lines: usize,
}

/// Scan `root` (optionally restricted to a `scope` path prefix) for duplicated
/// blocks. Returns the largest blocks first plus the total duplicated line
/// count across reported blocks.
pub fn scan_duplication(
    root: &Path,
    scope: Option<&str>,
    max_blocks: usize,
) -> (Vec<DupBlock>, usize) {
    let files = collect_source_files(root, scope);
    let mut hash_counts: HashMap<u64, usize> = HashMap::new();
    let mut per_file: Vec<(Vec<usize>, Vec<u64>)> = Vec::new();
    let mut file_paths: Vec<String> = Vec::new();
    let mut line_maps: Vec<Vec<usize>> = Vec::new();

    for path in &files {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        let (lines, map) = normalize_lines(&content);
        if lines.len() < WINDOW {
            continue;
        }
        let mut hashes = Vec::with_capacity(lines.len() - WINDOW + 1);
        for start in 0..=(lines.len() - WINDOW) {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            lines[start..start + WINDOW].hash(&mut hasher);
            let hash = hasher.finish();
            hashes.push(hash);
            *hash_counts.entry(hash).or_default() += 1;
        }
        file_paths.push(path.display().to_string());
        line_maps.push(map);
        per_file.push(((0..hashes.len()).collect(), hashes));
    }

    let mut segments: Vec<Segment> = Vec::new();
    for (file_idx, (positions, hashes)) in per_file.iter().enumerate() {
        let mut run: Vec<usize> = Vec::new();
        for (&window_idx, &hash) in positions.iter().zip(hashes.iter()) {
            if hash_counts.get(&hash).copied().unwrap_or(0) >= 2 {
                run.push(window_idx);
                continue;
            }
            if let Some(segment) = take_run(&mut run, WINDOW) {
                segments.push((file_idx, segment.0, segment.1));
            }
        }
        if let Some(segment) = take_run(&mut run, WINDOW) {
            segments.push((file_idx, segment.0, segment.1));
        }
    }
    segments.sort_unstable();
    segments.truncate(MAX_SEGMENTS);

    let mut candidates: Vec<DupBlock> = Vec::new();
    pair_segments(&segments, &file_paths, &line_maps, &mut candidates);

    let mut blocks = dedup_overlaps(candidates);
    blocks.sort_by_key(|block| std::cmp::Reverse(block.lines));
    blocks.truncate(max_blocks);
    let total = blocks.iter().map(|block| block.lines).sum();
    (blocks, total)
}

/// A maximal chain of matched windows inside one file:
/// `(file_idx, normalized_start, normalized_end_inclusive)`.
type Segment = (usize, usize, usize);

/// Flush the current run of consecutive window indices as a segment, or `None`
/// when the run is empty.
fn take_run(run: &mut Vec<usize>, window: usize) -> Option<(usize, usize)> {
    let last = *run.last()?;
    let first = run[0];
    run.clear();
    Some((first, last + window - 1))
}

/// Emit a block for every pair of segments that are in different files or
/// disjoint ranges of the same file; the duplicated extent is the shorter span.
fn pair_segments(
    segments: &[Segment],
    files: &[String],
    maps: &[Vec<usize>],
    out: &mut Vec<DupBlock>,
) {
    for i in 0..segments.len() {
        for j in (i + 1)..segments.len() {
            let (file_a, start_a, end_a) = segments[i];
            let (file_b, start_b, end_b) = segments[j];
            if file_a == file_b && start_a <= end_b && start_b <= end_a {
                continue;
            }
            let lines = (end_a - start_a + 1).min(end_b - start_b + 1);
            out.push(DupBlock {
                locations: vec![
                    DupLocation {
                        file: files[file_a].clone(),
                        start: maps[file_a][start_a],
                        end: maps[file_a][end_a],
                    },
                    DupLocation {
                        file: files[file_b].clone(),
                        start: maps[file_b][start_b],
                        end: maps[file_b][end_b],
                    },
                ],
                lines,
            });
        }
    }
}

/// Drop blocks whose span is contained in an already-accepted larger block in
/// the same file, keeping the maximal representatives.
fn dedup_overlaps(mut candidates: Vec<DupBlock>) -> Vec<DupBlock> {
    candidates.sort_by_key(|block| std::cmp::Reverse(block.lines));
    let mut accepted: Vec<DupBlock> = Vec::new();
    for block in candidates {
        let contained = accepted.iter().any(|kept| {
            block.locations.iter().all(|loc| {
                kept.locations
                    .iter()
                    .any(|k| k.file == loc.file && loc.start >= k.start && loc.end <= k.end)
            })
        });
        if !contained {
            accepted.push(block);
        }
    }
    accepted
}

/// Trim each line and drop blank and comment-only lines, keeping the original
/// line numbers so reported ranges point at real source.
fn normalize_lines(content: &str) -> (Vec<String>, Vec<usize>) {
    let mut lines = Vec::new();
    let mut map = Vec::new();
    for (idx, raw) in content.lines().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || is_comment_only(trimmed) {
            continue;
        }
        lines.push(trimmed.to_string());
        map.push(idx + 1);
    }
    (lines, map)
}

fn is_comment_only(line: &str) -> bool {
    ["//", "#", "/*", "*", "*/", "--", ";"]
        .iter()
        .any(|prefix| line.starts_with(prefix))
}

fn collect_source_files(root: &Path, scope: Option<&str>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy().to_string();
            if path.is_dir() {
                if !IGNORED_DIRS.contains(&name.as_str()) {
                    stack.push(path);
                }
                continue;
            }
            if !has_source_ext(&name) {
                continue;
            }
            if let Some(scope) = scope {
                if !in_scope(root, &path, scope) {
                    continue;
                }
            }
            files.push(path);
        }
    }
    files.sort();
    files
}

fn has_source_ext(name: &str) -> bool {
    name.contains('.')
        && name
            .rsplit('.')
            .next()
            .is_some_and(|ext| SOURCE_EXTS.contains(&ext))
}

fn in_scope(root: &Path, path: &Path, scope: &str) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let scope = scope.trim_matches('/');
    relative.starts_with(Path::new(scope)) || relative == Path::new(scope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("codecov-dup-{name}-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    const SHARED: &str = "\
fn shared_work(a: u32, b: u32) -> u32 {
    let sum = a + b;
    let doubled = sum * 2;
    let shifted = doubled << 1;
    let mixed = shifted ^ 0xff;
    let final_value = mixed - 7;
    final_value
}";
    const UNIQUE: &str = "\
fn totally_different() -> u32 {
    1 + 2 + 3 + 4
}";

    #[test]
    fn finds_cross_file_duplicates_with_line_numbers() {
        let root = temp_tree("cross");
        write(&root.join("src/a.rs"), &format!("{SHARED}\n{UNIQUE}\n"));
        write(
            &root.join("src/b.rs"),
            &format!("{SHARED}\nfn other() {{}}\n"),
        );
        let (blocks, total) = scan_duplication(&root, None, 20);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lines, 8);
        assert_eq!(total, 8);
        let files: Vec<&str> = blocks[0]
            .locations
            .iter()
            .map(|loc| loc.file.rsplit('/').next().unwrap())
            .collect();
        assert!(files.contains(&"a.rs") && files.contains(&"b.rs"));
        assert_eq!(blocks[0].locations[0].start, 1);
        assert_eq!(blocks[0].locations[0].end, 8);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unique_code_yields_no_blocks() {
        let root = temp_tree("unique");
        write(&root.join("src/a.rs"), UNIQUE);
        write(&root.join("src/b.rs"), "fn other_thing() -> u32 { 9 * 9 }");
        let (blocks, total) = scan_duplication(&root, None, 20);
        assert!(blocks.is_empty());
        assert_eq!(total, 0);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn ignores_build_dirs_and_non_source_files() {
        let root = temp_tree("ignore");
        write(&root.join("src/a.rs"), SHARED);
        write(&root.join("target/debug/generated.rs"), SHARED);
        write(&root.join("src/data.json"), "{}");
        let (blocks, _) = scan_duplication(&root, None, 20);
        assert!(blocks.is_empty());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scope_restricts_the_scan() {
        let root = temp_tree("scope");
        write(&root.join("src/a.rs"), &format!("{SHARED}\n{UNIQUE}\n"));
        write(
            &root.join("tests/b.rs"),
            &format!("{SHARED}\nfn t() {{}}\n"),
        );
        let (blocks, _) = scan_duplication(&root, Some("src"), 20);
        assert!(blocks.is_empty());
        let (blocks, _) = scan_duplication(&root, None, 20);
        assert_eq!(blocks.len(), 1);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn blank_and_comment_lines_do_not_split_blocks() {
        let root = temp_tree("comments");
        write(
            &root.join("src/a.rs"),
            &format!("{SHARED}\n\n// a comment\n{UNIQUE}\n"),
        );
        write(
            &root.join("src/b.rs"),
            &format!("// leading comment\n{SHARED}\nfn x() {{}}\n"),
        );
        let (blocks, total) = scan_duplication(&root, None, 20);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lines, 8);
        assert_eq!(total, 8);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn same_file_disjoint_duplicates_are_reported() {
        let root = temp_tree("self");
        write(
            &root.join("src/a.rs"),
            &format!("{SHARED}\nfn glue() {{}}\n{SHARED}\n"),
        );
        let (blocks, _) = scan_duplication(&root, None, 20);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].locations[0].file.ends_with("a.rs"));
        assert!(blocks[0].locations[1].file.ends_with("a.rs"));
        assert!(blocks[0].locations[0].end < blocks[0].locations[1].start);
        fs::remove_dir_all(&root).ok();
    }
}
