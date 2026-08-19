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
    pub created_at: u64,
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
        let entry = MemoryEntry {
            id: Uuid::new_v4().to_string(),
            session_key: session_key.into(),
            content: content.into(),
            tags,
            created_at: now_seconds(),
        };
        self.state.entries.insert(entry.id.clone(), entry.clone());
        self.persist()?;
        Ok(entry)
    }

    pub fn search(&self, session_key: Option<&str>, query: &str, limit: usize) -> Vec<MemoryEntry> {
        let query = query.to_ascii_lowercase();
        self.state
            .entries
            .values()
            .filter(|entry| session_key.is_none_or(|session| entry.session_key == session))
            .filter(|entry| {
                query.is_empty()
                    || entry.content.to_ascii_lowercase().contains(&query)
                    || entry
                        .tags
                        .iter()
                        .any(|tag| tag.to_ascii_lowercase().contains(&query))
            })
            .rev()
            .take(limit.max(1).min(100))
            .cloned()
            .collect()
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
        assert_eq!(store.search(Some("session-a"), "e2b", 10).len(), 1);
        assert_eq!(
            MemoryStore::load(&path)
                .unwrap()
                .search(None, "deployment", 10)
                .len(),
            1
        );
    }
}
