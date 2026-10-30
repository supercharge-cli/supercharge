//! Grove worktree operations. Create, remove, and status decline in this build.
#![allow(dead_code)]

use std::path::Path;

use anyhow::Result;

pub(crate) use crate::grove_api::is_safe_worktree_id;
pub use crate::grove_api::{
    CAP_CANCEL_WORKTREE_CREATE, CleanArtifactsReply, DetachReply, GroveHardFail, NfsAdopted,
    NfsCreateDecision, NfsStatusView, NfsWorktreeOpts, SalvageReply, daemon_capability_class,
    grove_hard_fail,
};
#[allow(unused_imports)] // re-exported for discovery / execute when those modules are on
pub(crate) use crate::grove_api::{default_grove_creation_mode, nfs_error_blocks_fallback};

pub(crate) use crate::worktree::GroveTry;

pub const WORKTREE_BACKING_DIR: &str = "worktree-backing";

pub fn dest_is_nfs_mount(_path: &Path) -> bool {
    false
}

#[cfg(windows)]
fn folded_components(path: &Path) -> Vec<String> {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

#[cfg(windows)]
pub(crate) fn dest_paths_equivalent(a: &Path, b: &Path) -> bool {
    folded_components(a) == folded_components(b)
}

#[cfg(windows)]
pub(crate) fn dest_path_contains(parent: &Path, child: &Path) -> bool {
    let parent = folded_components(parent);
    let child = folded_components(child);
    child.len() >= parent.len() && child[..parent.len()] == parent[..]
}

#[cfg(not(windows))]
pub(crate) fn dest_paths_equivalent(a: &Path, b: &Path) -> bool {
    a == b
}

#[cfg(not(windows))]
pub(crate) fn dest_path_contains(parent: &Path, child: &Path) -> bool {
    child.starts_with(parent)
}

#[derive(Debug, Clone)]
pub struct NfsWorktreeClient {
    control_sock: Option<std::path::PathBuf>,
}

impl NfsWorktreeClient {
    #[must_use]
    pub fn from_opts(opts: &NfsWorktreeOpts) -> Self {
        Self {
            control_sock: opts.control_sock.clone(),
        }
    }

    pub fn detach_worktree(&self, _dest: &Path, _allow_copy: bool) -> Result<DetachReply> {
        anyhow::bail!("not available in this build")
    }

    pub fn salvage_worktree(&self, _dest: &Path, _out: &Path) -> Result<SalvageReply> {
        anyhow::bail!("not available in this build")
    }

    pub fn clean_artifacts(&self, _dest: &Path) -> Result<CleanArtifactsReply> {
        anyhow::bail!("not available in this build")
    }

    /// Reads the daemon's redirect telemetry ring and acks every entry through
    /// `ack_through` (`0` acks nothing). Returns the entries and the next seq.
    /// This build ships no daemon of its own, so only an explicitly configured
    /// control socket is ever dialed.
    pub fn redirect_events(
        &self,
        ack_through: u64,
        timeout: std::time::Duration,
    ) -> Result<(Vec<serde_json::Value>, u64)> {
        let Some(sock) = self.control_sock.as_deref() else {
            anyhow::bail!("no Grove control socket is configured in this build")
        };
        let request = serde_json::json!({"op": "redirect_events", "v": 1, "ack_through": ack_through});
        let reply = control_request(sock, &request, timeout)?;
        let data = reply.get("data").cloned().unwrap_or_default();
        if reply.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
            let error = data.get("error").and_then(serde_json::Value::as_str).unwrap_or("unknown error");
            anyhow::bail!("redirect_events refused: {error}")
        }
        let events = match data.get("redirect_events") {
            Some(serde_json::Value::Array(events)) => events.clone(),
            _ => Vec::new(),
        };
        let next_seq = data
            .get("redirect_next_seq")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(ack_through.saturating_add(1));
        Ok((events, next_seq))
    }

    pub fn status_for_dir(&self, _dest: &Path) -> Option<NfsStatusView> {
        None
    }

    pub fn source_is_linked_local_view(&self, _source: &Path) -> bool {
        false
    }
}

#[must_use]
pub fn source_is_linked_local_view(_opts: &NfsWorktreeOpts, _source: &Path) -> bool {
    false
}

pub fn try_nfs_remove(_worktree_path: &Path) -> Result<Option<crate::RemoveReport>> {
    Ok(None)
}

/// A plan that asked for Grove records why it fell through, so the copy that
/// serves it still reports the request; a plan that did not ask skips silently.
pub(crate) fn try_grove_worktree(
    plan: &crate::worktree::plan::WorktreePlan,
) -> Result<Option<GroveTry>> {
    if plan.nfs.as_ref().is_some_and(|opts| opts.enabled) {
        return Ok(Some(GroveTry::Skipped(
            crate::worktree::GroveSkip::NotInThisBuild,
        )));
    }
    Ok(None)
}

/// One newline-delimited JSON request and reply on the daemon control socket.
#[cfg(unix)]
fn control_request(
    sock: &Path,
    request: &serde_json::Value,
    timeout: std::time::Duration,
) -> Result<serde_json::Value> {
    use std::io::{BufRead, BufReader, Write};
    let timeout = timeout.max(std::time::Duration::from_millis(1));
    let mut stream = std::os::unix::net::UnixStream::connect(sock)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    writeln!(stream, "{request}")?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim())?)
}

#[cfg(not(unix))]
fn control_request(
    _sock: &Path,
    _request: &serde_json::Value,
    _timeout: std::time::Duration,
) -> Result<serde_json::Value> {
    anyhow::bail!("the Grove control socket is unix-only in this build")
}

#[must_use]
pub(crate) fn probe_daemon_capability_class(
    _opts: Option<&NfsWorktreeOpts>,
) -> Option<&'static str> {
    None
}

pub fn dest_is_known_unmounted(_path: &Path) -> bool {
    true
}

pub fn dest_is_mountpoint(_path: &Path) -> bool {
    false
}

pub fn dest_is_projected_mount(_path: &Path) -> bool {
    false
}

#[cfg(feature = "metadata")]
mod metadata {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use anyhow::Result;

    use crate::db::WorktreeRecord;

    pub const RANK_DB: u8 = 0;

    #[derive(Debug, Clone)]
    pub struct NfsIdentity {
        pub worktree_id: String,
        pub dest: Option<PathBuf>,
        pub source_repo: Option<PathBuf>,
        pub pin_ref: Option<String>,
        pub backing: Option<PathBuf>,
        pub mount_id: Option<i64>,
        pub rank: u8,
        pub phase: Option<String>,
    }

    #[derive(Debug, Default)]
    pub struct PinGcReport {
        pub examined: u64,
        pub pruned: u64,
        pub deferred_grace: u64,
        pub kept_live: u64,
        pub pruned_ids: Vec<String>,
    }

    pub fn candidate_data_dirs() -> Vec<PathBuf> {
        Vec::new()
    }

    pub fn nfs_record_is_dead(dest: &Path, _backing: Option<&Path>) -> bool {
        if crate::nfs::dest_is_mountpoint(dest) || !crate::nfs::dest_is_known_unmounted(dest) {
            return false;
        }
        std::fs::symlink_metadata(dest).is_err()
    }

    pub fn identities_from_worktree_records(_recs: &[WorktreeRecord]) -> Vec<NfsIdentity> {
        Vec::new()
    }

    pub fn collect_identities(
        _data_dir: &Path,
        _worktrees: &[NfsIdentity],
    ) -> HashMap<String, NfsIdentity> {
        HashMap::new()
    }

    pub fn merge_nfs_identities(
        _into: &mut HashMap<String, NfsIdentity>,
        _src: impl IntoIterator<Item = NfsIdentity>,
    ) {
    }

    pub fn gc_orphan_pins(
        _data_dir: &Path,
        _worktrees: &[NfsIdentity],
        _now: i64,
        _dry_run: bool,
    ) -> Result<PinGcReport> {
        Ok(PinGcReport::default())
    }
}

#[cfg(feature = "metadata")]
pub use metadata::*;
