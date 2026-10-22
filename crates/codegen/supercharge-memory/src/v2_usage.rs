//! Created / last-used timestamps for memory-v2 notes.
//!
//! The ledger is a small JSON file in each scope root (`note_usage.json`),
//! keyed by scope-relative path such as `topics/build.md`. It is advisory:
//! concurrent sessions merge by keeping the newest timestamps, and a missing
//! or corrupt ledger falls back to file modification times.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const USAGE_LEDGER_FILE: &str = "note_usage.json";
const MAX_LEDGER_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct V2NoteUsage {
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

impl V2NoteUsage {
    /// Most recent sign of life: last use, else creation.
    pub fn last_activity(self) -> i64 {
        self.last_used_at.unwrap_or(self.created_at).max(self.created_at)
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct V2UsageLedger {
    entries: BTreeMap<String, V2NoteUsage>,
}

fn ledger_path(scope_dir: &Path) -> PathBuf {
    scope_dir.join(USAGE_LEDGER_FILE)
}

fn file_mtime(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let seconds = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    i64::try_from(seconds).ok()
}

impl V2UsageLedger {
    pub fn load(scope_dir: &Path) -> Self {
        let path = ledger_path(scope_dir);
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            return Self::default();
        };
        if !metadata.file_type().is_file() || metadata.len() > MAX_LEDGER_BYTES {
            return Self::default();
        }
        let value: serde_json::Value = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let entries = value
            .as_object()
            .map(|object| {
                object
                    .iter()
                    .filter_map(|(path, entry)| {
                        Some((
                            path.clone(),
                            V2NoteUsage {
                                created_at: entry.get("created_at")?.as_i64()?,
                                last_used_at: entry.get("last_used_at").and_then(|v| v.as_i64()),
                            },
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { entries }
    }

    pub fn get(&self, relative: &str) -> Option<V2NoteUsage> {
        self.entries.get(relative).copied()
    }

    /// Usage for a note, falling back to its modification time when the
    /// ledger has no entry.
    pub fn usage_or_mtime(&self, scope_dir: &Path, relative: &str) -> Option<V2NoteUsage> {
        self.get(relative).or_else(|| {
            file_mtime(&scope_dir.join(relative)).map(|created_at| V2NoteUsage {
                created_at,
                last_used_at: None,
            })
        })
    }

    /// Record creation if the note is new to the ledger. Returns whether it changed.
    pub fn ensure_created(&mut self, relative: &str, now: i64) -> bool {
        if self.entries.contains_key(relative) {
            return false;
        }
        self.entries.insert(
            relative.to_string(),
            V2NoteUsage {
                created_at: now,
                last_used_at: None,
            },
        );
        true
    }

    pub fn mark_used(&mut self, relative: &str, now: i64) {
        let entry = self
            .entries
            .entry(relative.to_string())
            .or_insert(V2NoteUsage {
                created_at: now,
                last_used_at: None,
            });
        entry.last_used_at = Some(entry.last_used_at.map_or(now, |seen| seen.max(now)));
    }

    pub fn remove(&mut self, relative: &str) {
        self.entries.remove(relative);
    }

    /// Merge with the ledger on disk (keeping the newest timestamps and the
    /// oldest creation) and write it atomically with owner-only permissions.
    pub fn save_merged(&self, scope_dir: &Path, removed: &[String]) -> std::io::Result<()> {
        let mut merged = Self::load(scope_dir);
        for (path, usage) in &self.entries {
            let entry = merged.entries.entry(path.clone()).or_insert(*usage);
            entry.created_at = entry.created_at.min(usage.created_at);
            entry.last_used_at = entry.last_used_at.max(usage.last_used_at);
        }
        for path in removed {
            merged.entries.remove(path);
        }
        let object: serde_json::Map<String, serde_json::Value> = merged
            .entries
            .iter()
            .map(|(path, usage)| {
                (
                    path.clone(),
                    serde_json::json!({
                        "created_at": usage.created_at,
                        "last_used_at": usage.last_used_at,
                    }),
                )
            })
            .collect();
        let bytes = serde_json::to_vec_pretty(&serde_json::Value::Object(object))
            .map_err(std::io::Error::other)?;
        let mut temporary = tempfile::NamedTempFile::new_in(scope_dir)?;
        std::io::Write::write_all(&mut temporary, &bytes)?;
        temporary.as_file().sync_all()?;
        let path = ledger_path(scope_dir);
        temporary.persist(&path).map_err(|error| error.error)?;
        crate::secret_guard::restrict_file(&path);
        Ok(())
    }
}

/// Mark notes used now. `paths` are absolute; ones outside `scope_dir` are ignored.
pub fn mark_notes_used(scope_dir: &Path, paths: &[PathBuf], now: i64) {
    let relatives: Vec<String> = paths
        .iter()
        .filter_map(|path| path.strip_prefix(scope_dir).ok())
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .collect();
    if relatives.is_empty() {
        return;
    }
    let mut ledger = V2UsageLedger::default();
    for relative in &relatives {
        ledger.mark_used(relative, now);
    }
    if let Err(error) = ledger.save_merged(scope_dir, &[]) {
        tracing::debug!(%error, "could not update memory note usage");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_oldest_creation_and_newest_use() {
        let temp = tempfile::tempdir().unwrap();
        let mut first = V2UsageLedger::default();
        first.ensure_created("topics/a.md", 100);
        first.save_merged(temp.path(), &[]).unwrap();
        let mut second = V2UsageLedger::default();
        second.mark_used("topics/a.md", 500);
        second.save_merged(temp.path(), &[]).unwrap();
        let loaded = V2UsageLedger::load(temp.path());
        assert_eq!(
            loaded.get("topics/a.md"),
            Some(V2NoteUsage {
                created_at: 100,
                last_used_at: Some(500)
            })
        );
        mark_notes_used(temp.path(), &[temp.path().join("topics/a.md")], 400);
        let loaded = V2UsageLedger::load(temp.path());
        assert_eq!(loaded.get("topics/a.md").unwrap().last_activity(), 500);
        V2UsageLedger::default()
            .save_merged(temp.path(), &["topics/a.md".into()])
            .unwrap();
        assert!(V2UsageLedger::load(temp.path()).get("topics/a.md").is_none());
    }

    #[test]
    fn corrupt_ledger_is_ignored() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(USAGE_LEDGER_FILE), "{not json").unwrap();
        assert_eq!(V2UsageLedger::load(temp.path()), V2UsageLedger::default());
    }
}
