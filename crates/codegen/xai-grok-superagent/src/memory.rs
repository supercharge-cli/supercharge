use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryEntry {
    pub id: String,
    pub session_key: String,
    pub content: String,
    pub tags: Vec<String>,
    #[serde(default = "default_memory_kind")]
    pub kind: String,
    #[serde(default = "default_memory_importance")]
    pub importance: u8,
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

fn default_memory_kind() -> String {
    "note".to_owned()
}

fn default_memory_importance() -> u8 {
    50
}

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("memory store I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("memory store JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedMemory {
    entries: BTreeMap<String, MemoryEntry>,
}

pub struct MemoryStore {
    path: PathBuf,
    state: PersistedMemory,
}

impl MemoryStore {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, MemoryError> {
        let path = path.into();
        let state = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            PersistedMemory::default()
        };
        Ok(Self { path, state })
    }

    pub fn remember(
        &mut self,
        session_key: impl Into<String>,
        content: impl Into<String>,
        tags: Vec<String>,
    ) -> Result<MemoryEntry, MemoryError> {
        self.remember_with_metadata(session_key, content, tags, "note", 50)
    }

    pub fn remember_with_metadata(
        &mut self,
        session_key: impl Into<String>,
        content: impl Into<String>,
        tags: Vec<String>,
        kind: impl Into<String>,
        importance: u8,
    ) -> Result<MemoryEntry, MemoryError> {
        let now = now_seconds();
        let entry = MemoryEntry {
            id: Uuid::new_v4().to_string(),
            session_key: session_key.into(),
            content: content.into(),
            tags,
            kind: kind.into(),
            importance,
            created_at: now,
            updated_at: now,
        };
        self.state.entries.insert(entry.id.clone(), entry.clone());
        self.persist()?;
        Ok(entry)
    }

    pub fn search(&self, session_key: Option<&str>, query: &str, limit: usize) -> Vec<MemoryEntry> {
        let query = query.to_ascii_lowercase();
        let terms: Vec<&str> = query
            .split_whitespace()
            .filter(|term| !term.is_empty())
            .collect();
        let mut matches: Vec<(u32, &MemoryEntry)> = self
            .state
            .entries
            .values()
            .filter(|entry| session_key.is_none_or(|session| entry.session_key == session))
            .filter_map(|entry| {
                let content = entry.content.to_ascii_lowercase();
                let tags = entry.tags.join(" ").to_ascii_lowercase();
                let kind = entry.kind.to_ascii_lowercase();
                let score = if terms.is_empty() {
                    1
                } else {
                    terms.iter().fold(0u32, |score, term| {
                        score
                            + u32::from(content.contains(term)) * 5
                            + u32::from(tags.contains(term)) * 3
                            + u32::from(kind.contains(term)) * 2
                    })
                };
                (score > 0).then_some((score + u32::from(entry.importance), entry))
            })
            .collect();
        matches.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
        });
        matches
            .into_iter()
            .take(limit.clamp(1, 100))
            .map(|(_, entry)| entry.clone())
            .collect()
    }

    pub fn delete(&mut self, id: &str) -> Result<bool, MemoryError> {
        let removed = self.state.entries.remove(id).is_some();
        if removed {
            self.persist()?;
        }
        Ok(removed)
    }

    pub fn delete_session(&mut self, session_key: &str) -> Result<usize, MemoryError> {
        let before = self.state.entries.len();
        self.state
            .entries
            .retain(|_, entry| entry.session_key != session_key);
        let removed = before - self.state.entries.len();
        if removed > 0 {
            self.persist()?;
        }
        Ok(removed)
    }

    pub fn compact(&mut self, max_entries: usize) -> Result<usize, MemoryError> {
        let max_entries = max_entries.max(1);
        if self.state.entries.len() <= max_entries {
            return Ok(0);
        }
        let mut entries: Vec<_> = self.state.entries.values().cloned().collect();
        entries.sort_by(|left, right| {
            left.importance
                .cmp(&right.importance)
                .then_with(|| left.updated_at.cmp(&right.updated_at))
        });
        let remove_count = entries.len() - max_entries;
        for entry in entries.into_iter().take(remove_count) {
            self.state.entries.remove(&entry.id);
        }
        self.persist()?;
        Ok(remove_count)
    }

    pub fn export(&self) -> Result<Vec<MemoryEntry>, MemoryError> {
        Ok(self.state.entries.values().cloned().collect())
    }

    pub fn restore(
        &mut self,
        entries: Vec<MemoryEntry>,
        replace: bool,
    ) -> Result<usize, MemoryError> {
        if replace {
            self.state.entries.clear();
        }
        let mut inserted = 0;
        for entry in entries {
            if entry.session_key.trim().is_empty() || entry.content.trim().is_empty() {
                continue;
            }
            self.state.entries.insert(entry.id.clone(), entry);
            inserted += 1;
        }
        self.persist()?;
        Ok(inserted)
    }

    pub fn stats(&self) -> (usize, usize) {
        let sessions = self
            .state
            .entries
            .values()
            .map(|entry| entry.session_key.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        (self.state.entries.len(), sessions)
    }

    fn persist(&self) -> Result<(), MemoryError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = self.path.with_extension("tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(&self.state)?)?;
        std::fs::rename(temp, &self.path)?;
        Ok(())
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::MemoryStore;
    use tempfile::tempdir;

    #[test]
    fn memory_survives_restart_and_searches_by_session_and_tag() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.json");
        let mut store = MemoryStore::load(&path).unwrap();
        store
            .remember(
                "session-a",
                "Rust deployment uses E2B",
                vec!["deployment".into()],
            )
            .unwrap();
        store.remember("session-b", "unrelated", vec![]).unwrap();
        store
            .remember_with_metadata(
                "session-a",
                "Critical deployment preference",
                vec!["preference".into()],
                "preference",
                100,
            )
            .unwrap();
        assert_eq!(
            store.search(Some("session-a"), "deployment", 10)[0].kind,
            "preference"
        );
        assert_eq!(store.search(Some("session-a"), "e2b", 10).len(), 1);
        let mut restored = MemoryStore::load(&path).unwrap();
        assert_eq!(restored.search(None, "deployment", 10).len(), 2);
        assert_eq!(restored.stats(), (3, 2));
        assert!(restored.delete_session("session-b").unwrap() > 0);
        assert_eq!(restored.stats(), (2, 1));
        let backup = restored.export().unwrap();
        let restore_path = dir.path().join("restored.json");
        let mut imported = MemoryStore::load(&restore_path).unwrap();
        assert_eq!(imported.restore(backup, false).unwrap(), 2);
        assert_eq!(imported.stats(), (2, 1));
    }
}
