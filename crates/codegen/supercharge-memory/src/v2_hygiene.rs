//! Deterministic topic hygiene run after Dream: merge near-duplicate topic
//! notes and archive notes nobody has used for `stale_days`.
//!
//! Archived notes move to `archive/topics/` (not deleted), so a user can
//! restore one by moving it back.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use crate::V2MemoryScope;
use crate::v2_dedupe::{jaccard, note_word_set};
use crate::v2_usage::{V2NoteUsage, V2UsageLedger};

const MAX_TOPICS: usize = 512;
const MAX_TOPIC_BYTES: u64 = 256 * 1024;
const MIN_MERGE_WORDS: usize = 4;
const SECONDS_PER_DAY: i64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct V2HygienePolicy {
    /// Archive topics unused for longer than this. `0` disables archiving.
    pub stale_days: u64,
    /// Word-set overlap (Jaccard) at which two topics count as duplicates.
    pub merge_threshold: f64,
}

impl Default for V2HygienePolicy {
    fn default() -> Self {
        Self {
            stale_days: 90,
            merge_threshold: 0.8,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct V2HygieneReport {
    /// `(kept, merged_away)` pairs; the second file was archived.
    pub merged: Vec<(PathBuf, PathBuf)>,
    pub archived_stale: Vec<PathBuf>,
}

impl V2HygieneReport {
    pub fn is_empty(&self) -> bool {
        self.merged.is_empty() && self.archived_stale.is_empty()
    }
}

struct Topic {
    relative: String,
    path: PathBuf,
    content: String,
    words: BTreeSet<String>,
    usage: V2NoteUsage,
}

fn load_topics(scope_dir: &Path, ledger: &V2UsageLedger) -> Vec<Topic> {
    let excluded = crate::v2::excluded_manifest_paths(scope_dir).unwrap_or_default();
    let Ok(entries) = std::fs::read_dir(scope_dir.join("topics")) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    paths.sort();
    paths.truncate(MAX_TOPICS);
    paths
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_string();
            let relative = format!("topics/{name}");
            if excluded.contains(&relative)
                || std::fs::metadata(&path).ok()?.len() > MAX_TOPIC_BYTES
            {
                return None;
            }
            let content = std::fs::read_to_string(&path).ok()?;
            let usage = ledger.usage_or_mtime(scope_dir, &relative)?;
            Some(Topic {
                words: note_word_set(&content),
                relative,
                path,
                content,
                usage,
            })
        })
        .collect()
}

fn archive_topic(scope_dir: &Path, topic: &Topic, now: i64) -> std::io::Result<PathBuf> {
    let archive_dir = scope_dir.join("archive/topics");
    std::fs::create_dir_all(&archive_dir)?;
    crate::secret_guard::restrict_dir_chain(scope_dir, &archive_dir);
    let stem = topic
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("topic");
    let mut target = archive_dir.join(format!("{stem}.{now}.md"));
    let mut suffix = 1;
    while target.exists() {
        target = archive_dir.join(format!("{stem}.{now}-{suffix}.md"));
        suffix += 1;
    }
    std::fs::rename(&topic.path, &target)?;
    Ok(target)
}

fn line_key(line: &str) -> String {
    note_word_set(line).into_iter().collect::<Vec<_>>().join(" ")
}

/// Append lines of `other` that `keeper` does not already contain.
fn merge_contents(keeper: &str, other: &str, other_name: &str) -> String {
    let known: HashSet<String> = keeper.lines().map(line_key).collect();
    let body = other
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n").map(|(_, body)| body))
        .unwrap_or(other);
    let extra: Vec<&str> = body
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .filter(|line| {
            let key = line_key(line);
            !key.is_empty() && !known.contains(&key)
        })
        .collect();
    let mut merged = keeper.trim_end().to_string();
    if !extra.is_empty() {
        merged.push_str(&format!("\n\n## Merged from {other_name}\n"));
        for line in extra {
            merged.push_str(line);
            merged.push('\n');
        }
    } else {
        merged.push('\n');
    }
    merged
}

fn write_atomically(path: &Path, contents: &str) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("topic has no parent"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(&mut temporary, contents.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    crate::secret_guard::restrict_file(path);
    Ok(())
}

/// Merge near-duplicate topics and archive stale ones in one scope.
///
/// # Errors
///
/// Returns an I/O error when a merge or archive move fails; work finished
/// before the failure stays applied.
pub fn run_topic_hygiene(
    scope_dir: &Path,
    scope: V2MemoryScope,
    now: i64,
    policy: V2HygienePolicy,
) -> std::io::Result<V2HygieneReport> {
    let mut ledger = V2UsageLedger::load(scope_dir);
    let mut topics = load_topics(scope_dir, &ledger);
    let mut report = V2HygieneReport::default();
    let mut removed = Vec::new();

    if policy.stale_days > 0 {
        let cutoff = now.saturating_sub(
            i64::try_from(policy.stale_days)
                .unwrap_or(i64::MAX / SECONDS_PER_DAY)
                .saturating_mul(SECONDS_PER_DAY),
        );
        let (stale, fresh): (Vec<Topic>, Vec<Topic>) = topics
            .into_iter()
            .partition(|topic| topic.usage.last_activity() < cutoff);
        topics = fresh;
        for topic in stale {
            archive_topic(scope_dir, &topic, now)?;
            removed.push(topic.relative.clone());
            report.archived_stale.push(topic.path);
        }
    }

    let mut gone = vec![false; topics.len()];
    for left in 0..topics.len() {
        if gone[left] || topics[left].words.len() < MIN_MERGE_WORDS {
            continue;
        }
        for right in (left + 1)..topics.len() {
            if gone[right] || gone[left] || topics[right].words.len() < MIN_MERGE_WORDS {
                continue;
            }
            if jaccard(&topics[left].words, &topics[right].words) < policy.merge_threshold {
                continue;
            }
            let (keep, drop) =
                if topics[right].usage.last_activity() > topics[left].usage.last_activity() {
                    (right, left)
                } else {
                    (left, right)
                };
            let drop_name = topics[drop]
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("topic")
                .to_string();
            let merged = merge_contents(&topics[keep].content, &topics[drop].content, &drop_name);
            if merged.len() as u64 > MAX_TOPIC_BYTES {
                continue;
            }
            write_atomically(&topics[keep].path, &merged)?;
            archive_topic(scope_dir, &topics[drop], now)?;
            let usage = V2NoteUsage {
                created_at: topics[keep].usage.created_at.min(topics[drop].usage.created_at),
                last_used_at: topics[keep].usage.last_used_at.max(topics[drop].usage.last_used_at),
            };
            topics[keep].content = merged;
            topics[keep].words = note_word_set(&topics[keep].content);
            topics[keep].usage = usage;
            ledger.remove(&topics[keep].relative);
            ledger.ensure_created(&topics[keep].relative, usage.created_at);
            if let Some(used) = usage.last_used_at {
                ledger.mark_used(&topics[keep].relative, used);
            }
            removed.push(topics[drop].relative.clone());
            report
                .merged
                .push((topics[keep].path.clone(), topics[drop].path.clone()));
            gone[drop] = true;
        }
    }

    if !report.is_empty() {
        for relative in &removed {
            ledger.remove(relative);
        }
        ledger.save_merged(scope_dir, &removed)?;
        crate::v2::bump_manifest_revision(scope_dir).map_err(std::io::Error::other)?;
        crate::v2::regenerate_scope_manifest(
            scope_dir,
            scope,
            crate::v2::V2ManifestBudget::default(),
        )
        .map_err(std::io::Error::other)?;
    }
    Ok(report)
}

#[cfg(test)]
#[path = "v2_hygiene_tests.rs"]
mod tests;
