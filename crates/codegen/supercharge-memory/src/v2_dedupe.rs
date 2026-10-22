//! Duplicate detection for captured memory-v2 observations.
//!
//! A statement is a duplicate when its normalized words hash to one already in
//! memory, or when its word set overlaps an existing line by at least
//! [`OVERLAP_THRESHOLD`] (Jaccard). Existing lines come from topic files and
//! inbox observations, bounded in count and size.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

const OVERLAP_THRESHOLD: f64 = 0.8;
/// Short lines match too loosely by overlap; they only match exactly.
const MIN_OVERLAP_WORDS: usize = 4;
const MAX_FILES: usize = 4_096;
const MAX_FILE_BYTES: u64 = 64 * 1024;

pub(crate) struct KnownStatements {
    hashes: HashSet<blake3::Hash>,
    word_sets: Vec<BTreeSet<String>>,
}

impl KnownStatements {
    /// Load statements from `topics/` and `observations/_inbox/`, skipping
    /// inbox files whose name starts with `skip_prefix` (the current job).
    pub(crate) fn load(scope_dir: &Path, skip_prefix: &str) -> Self {
        let mut known = Self {
            hashes: HashSet::new(),
            word_sets: Vec::new(),
        };
        for (relative, skip) in [("topics", None), ("observations/_inbox", Some(skip_prefix))] {
            let Ok(entries) = std::fs::read_dir(scope_dir.join(relative)) else {
                continue;
            };
            for entry in entries.flatten().take(MAX_FILES) {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if path.extension().and_then(|ext| ext.to_str()) != Some("md")
                    || skip.is_some_and(|prefix| name.starts_with(prefix))
                {
                    continue;
                }
                let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(&path) {
                    for line in body_lines(&text) {
                        known.add(line);
                    }
                }
            }
        }
        known
    }

    fn add(&mut self, line: &str) {
        let words = words(line);
        if words.is_empty() {
            return;
        }
        self.hashes.insert(hash_words(&words));
        if words.len() >= MIN_OVERLAP_WORDS {
            self.word_sets.push(words.into_iter().collect());
        }
    }

    pub(crate) fn is_duplicate(&self, statement: &str) -> bool {
        let words = words(statement);
        if words.is_empty() {
            return false;
        }
        if self.hashes.contains(&hash_words(&words)) {
            return true;
        }
        if words.len() < MIN_OVERLAP_WORDS {
            return false;
        }
        let set: BTreeSet<String> = words.into_iter().collect();
        self.word_sets.iter().any(|known| jaccard(&set, known) >= OVERLAP_THRESHOLD)
    }

    /// Record `statement` and return true when it was not already known.
    pub(crate) fn insert_if_new(&mut self, statement: &str) -> bool {
        if self.is_duplicate(statement) {
            return false;
        }
        self.add(statement);
        true
    }
}

/// Markdown lines after any front matter, without heading or bullet markers.
fn body_lines(text: &str) -> impl Iterator<Item = &str> {
    let body = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n").map(|(_, body)| body))
        .unwrap_or(text);
    body.lines().map(|line| {
        line.trim()
            .trim_start_matches('#')
            .trim_start_matches(['-', '*'])
            .trim()
    })
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Word set of a whole note body (front matter and list markers stripped).
pub(crate) fn note_word_set(text: &str) -> BTreeSet<String> {
    body_lines(text).flat_map(words).collect()
}

fn hash_words(words: &[String]) -> blake3::Hash {
    blake3::hash(words.join(" ").as_bytes())
}

pub(crate) fn jaccard(left: &BTreeSet<String>, right: &BTreeSet<String>) -> f64 {
    let union = left.union(right).count();
    if union == 0 {
        return 0.0;
    }
    left.intersection(right).count() as f64 / union as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope_with(topic: &str, inbox: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("topics")).unwrap();
        std::fs::create_dir_all(dir.path().join("observations/_inbox")).unwrap();
        std::fs::write(dir.path().join("topics/build.md"), topic).unwrap();
        for (name, body) in inbox {
            std::fs::write(dir.path().join("observations/_inbox").join(name), body).unwrap();
        }
        dir
    }

    #[test]
    fn exact_and_near_duplicates_are_detected() {
        let dir = scope_with(
            "# Build\nHow to build.\n- Run cargo with CARGO_BUILD_JOBS=1 on the shared box\n",
            &[],
        );
        let known = KnownStatements::load(dir.path(), "job__");
        assert!(known.is_duplicate("run cargo with cargo_build_jobs=1 on the shared box."));
        assert!(known.is_duplicate("Always run cargo with CARGO_BUILD_JOBS=1 on the shared box"));
        assert!(!known.is_duplicate("The user prefers short answers"));
    }

    #[test]
    fn short_statements_only_match_exactly() {
        let dir = scope_with("# Style\n- Use tabs\n", &[]);
        let known = KnownStatements::load(dir.path(), "job__");
        assert!(known.is_duplicate("use tabs"));
        assert!(!known.is_duplicate("use spaces"));
    }

    #[test]
    fn own_job_files_are_ignored_and_front_matter_is_skipped() {
        let dir = scope_with(
            "",
            &[
                ("job__n000.md", "---\njob_id: x\n---\n\n# The deploy key lives in the vault\n"),
                ("other__n000.md", "---\ntopic_hint: null\n---\n\n# Tests need RUST_MIN_STACK raised\n"),
            ],
        );
        let known = KnownStatements::load(dir.path(), "job__");
        assert!(!known.is_duplicate("The deploy key lives in the vault"));
        assert!(known.is_duplicate("Tests need RUST_MIN_STACK raised"));
        assert!(!known.is_duplicate("job id x"));
    }

    #[test]
    fn batch_repeats_are_dropped() {
        let dir = scope_with("", &[]);
        let mut known = KnownStatements::load(dir.path(), "job__");
        assert!(known.insert_if_new("The user deploys from the main branch only"));
        assert!(!known.insert_if_new("The user deploys from the main branch only!"));
    }
}
