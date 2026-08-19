use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatewayMessage {
    pub platform: String,
    pub update_id: i64,
    pub chat_id: i64,
    pub user_id: Option<i64>,
    pub thread_id: Option<i64>,
    pub text: String,
}

impl GatewayMessage {
    pub fn session_key(&self) -> SessionKey {
        SessionKey::new(
            &self.platform,
            if self.thread_id.is_some() {
                "thread"
            } else {
                "chat"
            },
            self.thread_id.unwrap_or(self.chat_id),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionKey(String);

impl SessionKey {
    pub fn new(platform: &str, chat_type: &str, chat_id: i64) -> Self {
        Self(format!("agent:main:{platform}:{chat_type}:{chat_id}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatewayConfig {
    #[serde(default)]
    pub allowed_users: BTreeSet<i64>,
    #[serde(default)]
    pub allowed_chats: BTreeSet<i64>,
    #[serde(default = "default_true")]
    pub require_user_or_chat_allowlist: bool,
    #[serde(default = "default_true")]
    pub require_mention_in_groups: bool,
}

const fn default_true() -> bool {
    true
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            allowed_users: BTreeSet::new(),
            allowed_chats: BTreeSet::new(),
            require_user_or_chat_allowlist: true,
            require_mention_in_groups: true,
        }
    }
}

impl GatewayConfig {
    pub fn authorized(&self, user_id: Option<i64>, chat_id: i64) -> bool {
        if !self.require_user_or_chat_allowlist {
            return true;
        }
        user_id.is_some_and(|id| self.allowed_users.contains(&id))
            || self.allowed_chats.contains(&chat_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PairingEntry {
    code: String,
    expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedGatewayState {
    update_offset: Option<i64>,
    pairings: BTreeMap<i64, PairingEntry>,
}

#[derive(Debug, Error)]
pub enum GatewayStateError {
    #[error("gateway state I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("gateway state JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct GatewayStateStore {
    path: PathBuf,
    state: PersistedGatewayState,
}

impl GatewayStateStore {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, GatewayStateError> {
        let path = path.into();
        let state = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            PersistedGatewayState::default()
        };
        Ok(Self { path, state })
    }

    pub fn update_offset(&self) -> Option<i64> {
        self.state.update_offset
    }

    pub fn set_update_offset(&mut self, offset: i64) -> Result<(), GatewayStateError> {
        self.state.update_offset = Some(offset);
        self.persist()
    }

    pub fn create_pairing_code(
        &mut self,
        user_id: i64,
        ttl_seconds: u64,
    ) -> Result<String, GatewayStateError> {
        let code = Uuid::new_v4().simple().to_string()[..8].to_ascii_uppercase();
        self.state.pairings.insert(
            user_id,
            PairingEntry {
                code: code.clone(),
                expires_at: now_seconds().saturating_add(ttl_seconds),
            },
        );
        self.persist()?;
        Ok(code)
    }

    pub fn redeem_pairing_code(
        &mut self,
        user_id: i64,
        code: &str,
    ) -> Result<bool, GatewayStateError> {
        let valid = self
            .state
            .pairings
            .get(&user_id)
            .is_some_and(|entry| entry.expires_at >= now_seconds() && entry.code == code);
        if valid {
            self.state.pairings.remove(&user_id);
            self.persist()?;
        }
        Ok(valid)
    }

    fn persist(&self) -> Result<(), GatewayStateError> {
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

#[allow(dead_code)]
fn _path_exists(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{GatewayConfig, GatewayMessage, GatewayStateStore};

    #[test]
    fn session_key_includes_platform_and_thread_context() {
        let message = GatewayMessage {
            platform: "telegram".into(),
            update_id: 1,
            chat_id: 42,
            user_id: Some(7),
            thread_id: Some(9),
            text: "hello".into(),
        };
        assert_eq!(
            message.session_key().as_str(),
            "agent:main:telegram:thread:9"
        );
    }

    #[test]
    fn state_store_persists_offset_and_single_use_pairing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("gateway.json");
        let mut store = GatewayStateStore::load(&path).unwrap();
        store.set_update_offset(12).unwrap();
        let code = store.create_pairing_code(7, 60).unwrap();
        assert!(store.redeem_pairing_code(7, &code).unwrap());
        assert!(!store.redeem_pairing_code(7, &code).unwrap());
        let restored = GatewayStateStore::load(&path).unwrap();
        assert_eq!(restored.update_offset(), Some(12));
    }

    #[test]
    fn default_authorization_is_deny_by_default() {
        assert!(!GatewayConfig::default().authorized(Some(7), 42));
    }
}
