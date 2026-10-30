//! Cross-process session change feed for ACP clients.
//!
//! Polls `<SUPERCHARGE_HOME>/sessions/*/*/summary.json` modification times and reports
//! created / updated / deleted sessions as `_supercharge/sessions/changed`. Read-only: every
//! write still goes through the CLI's own session storage.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Wire name (sent with a leading `_` as an ACP extension notification).
pub const SUPERCHARGE_SESSIONS_CHANGED_METHOD: &str = "supercharge/sessions/changed";

/// Poll interval for the summary scan.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Created,
    Updated,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionChange {
    pub kind: ChangeKind,
    pub session_id: String,
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub mtime: SystemTime,
    pub cwd: String,
}

/// `session id -> (mtime, cwd)` for every summary under `sessions_root`.
pub fn scan(sessions_root: &Path) -> HashMap<String, Seen> {
    let mut out = HashMap::new();
    let Ok(projects) = std::fs::read_dir(sessions_root) else {
        return out;
    };
    for project in projects.flatten() {
        let encoded = project.file_name().to_string_lossy().into_owned();
        let cwd = urlencoding::decode(&encoded)
            .map(|c| c.into_owned())
            .unwrap_or(encoded);
        let Ok(sessions) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for session in sessions.flatten() {
            let summary = session.path().join(crate::session::storage::SUMMARY_FILE);
            if let Ok(mtime) = std::fs::metadata(&summary).and_then(|m| m.modified()) {
                out.insert(
                    session.file_name().to_string_lossy().into_owned(),
                    Seen {
                        mtime,
                        cwd: cwd.clone(),
                    },
                );
            }
        }
    }
    out
}

/// Changes between two scans, sorted by session id for stable output.
pub fn diff(before: &HashMap<String, Seen>, after: &HashMap<String, Seen>) -> Vec<SessionChange> {
    let mut changes = Vec::new();
    for (id, now) in after {
        let kind = match before.get(id) {
            None => Some(ChangeKind::Created),
            Some(prev) if prev.mtime != now.mtime => Some(ChangeKind::Updated),
            Some(_) => None,
        };
        if let Some(kind) = kind {
            changes.push(SessionChange {
                kind,
                session_id: id.clone(),
                cwd: now.cwd.clone(),
            });
        }
    }
    for (id, prev) in before {
        if !after.contains_key(id) {
            changes.push(SessionChange {
                kind: ChangeKind::Deleted,
                session_id: id.clone(),
                cwd: prev.cwd.clone(),
            });
        }
    }
    changes.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    changes
}

/// Default sessions directory under the Supercharge home.
pub fn default_sessions_root() -> PathBuf {
    supercharge_dirs::supercharge_home().join("sessions")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_summary(root: &Path, cwd: &str, id: &str) {
        let dir = root.join(urlencoding::encode(cwd).as_ref()).join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("summary.json"), "{}").unwrap();
    }

    #[test]
    fn reports_created_updated_and_deleted_with_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_summary(root, "/work/a b", "s1");
        write_summary(root, "/work/c", "s2");
        let first = scan(root);
        assert_eq!(first["s1"].cwd, "/work/a b");

        let mut later = first.clone();
        later.get_mut("s1").unwrap().mtime += std::time::Duration::from_secs(1);
        later.remove("s2");
        later.insert(
            "s3".into(),
            Seen {
                mtime: SystemTime::UNIX_EPOCH,
                cwd: "/work/d".into(),
            },
        );
        let changes = diff(&first, &later);
        assert_eq!(
            changes,
            vec![
                SessionChange {
                    kind: ChangeKind::Updated,
                    session_id: "s1".into(),
                    cwd: "/work/a b".into()
                },
                SessionChange {
                    kind: ChangeKind::Deleted,
                    session_id: "s2".into(),
                    cwd: "/work/c".into()
                },
                SessionChange {
                    kind: ChangeKind::Created,
                    session_id: "s3".into(),
                    cwd: "/work/d".into()
                },
            ]
        );
        assert!(diff(&first, &first).is_empty());
    }

    #[test]
    fn missing_root_scans_empty() {
        assert!(scan(Path::new("/nonexistent/supercharge/sessions")).is_empty());
    }
}
