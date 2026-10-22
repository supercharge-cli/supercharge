//! Filesystem access policy for model-visible memory v2 roots.
//!
//! Reads may inspect safe in-scope files. Writes are limited to Markdown topic
//! and observation-inbox files, use atomic replacement, and require an
//! unchanged snapshot from an earlier ordinary read when replacing a file.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};

use supercharge_tools::types::memory_v2::{MemoryV2Access, MemoryV2Write};

use crate::v2::{
    V2ManifestBudget, V2MemoryScope, V2StorageError, bump_manifest_revision,
    bump_manifest_revision_in_transaction, is_durably_excluded, regenerate_scope_manifest,
};

const MAX_PREVIOUS_CONTENT_BYTES: u64 = 8 * 1024 * 1024;
/// Largest topic a write may produce. Dream's `read_topics` and forget both
/// refuse larger files, so accepting one here would make every later Dream fail.
const MAX_TOPIC_FILE_BYTES: u64 = 256 * 1024;
/// Largest inbox observation a write may produce. Mirrors the private
/// `MAX_OBSERVATION_FILE_BYTES` in `v2_capture.rs`, which is the source of
/// truth for what capture recovery and Dream are willing to read back.
const MAX_WRITE_OBSERVATION_BYTES: u64 = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V2PathClass {
    Outside,
    Manifest(V2MemoryScope),
    Topic(V2MemoryScope),
    Observation(V2MemoryScope),
    ArchivedObservation(V2MemoryScope),
    /// A path nested below `topics/` or `observations/_inbox/`. Manifest
    /// generation, browse, Dream input, and capture recovery only list
    /// immediate children, so such files would be invisible.
    Nested(V2MemoryScope),
    Protected(V2MemoryScope),
}

impl V2PathClass {
    fn scope(self) -> Option<V2MemoryScope> {
        match self {
            Self::Outside => None,
            Self::Manifest(scope)
            | Self::Topic(scope)
            | Self::Observation(scope)
            | Self::Nested(scope)
            | Self::ArchivedObservation(scope)
            | Self::Protected(scope) => Some(scope),
        }
    }

    fn is_writable(self) -> bool {
        matches!(self, Self::Topic(_) | Self::Observation(_))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum V2AccessError {
    #[error("memory v2 path must be absolute: {0}")]
    RelativePath(PathBuf),
    #[error("memory v2 path contains traversal components: {0}")]
    Traversal(PathBuf),
    #[error("memory v2 path escapes its configured scope: {0}")]
    EscapesScope(PathBuf),
    #[error("symbolic links are not allowed in memory v2 paths: {0}")]
    Symlink(PathBuf),
    #[error("failed to inspect memory v2 path {path}: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("writes are not allowed to protected memory v2 path: {0}")]
    Protected(PathBuf),
    #[error("memory files must live directly under topics/ or observations/_inbox/: {0}")]
    NestedPath(PathBuf),
    #[error("memory v2 writes require a .md file: {0}")]
    NonMarkdown(PathBuf),
    #[error("read {0} before editing it")]
    ReadRequired(PathBuf),
    #[error("memory v2 file changed since it was read; read it again before editing: {0}")]
    Stale(PathBuf),
    #[error("memory v2 file is too large to edit safely (limit: {limit_bytes} bytes): {path}")]
    TooLarge { path: PathBuf, limit_bytes: u64 },
    #[error(
        "refused to save {path}: it looks like it contains a secret ({kind}). Memory must never store API keys, tokens, passwords, or other credentials; remove the value and save only a description of it."
    )]
    Secret { path: PathBuf, kind: &'static str },
    #[error(
        "refused to add {path}: it nearly duplicates the existing note {existing}. Read {existing} and edit it (replace the outdated lines) instead of adding a new note."
    )]
    NearDuplicate { path: PathBuf, existing: PathBuf },
    #[error(
        "the project note is limited to {limit} characters (got {chars}); keep only the most useful facts: {path}"
    )]
    ProjectNoteTooLong {
        path: PathBuf,
        limit: usize,
        chars: usize,
    },
    #[error("memory v2 path is durably excluded and cannot be recreated: {0}")]
    Excluded(PathBuf),
    #[error(
        "memory v2 observations are append-only; create a new observation file or record the correction in a topic instead of editing {0}"
    )]
    ImmutableObservation(PathBuf),
    #[error("failed to inspect memory v2 durable exclusions for {path}: {source}")]
    ExclusionState {
        path: PathBuf,
        #[source]
        source: V2StorageError,
    },
    #[error("failed to atomically write memory v2 file {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "memory file write was rolled back because its generated MEMORY.md manifest could not be refreshed: {source}"
    )]
    ManifestRefresh {
        #[source]
        source: V2StorageError,
    },
    #[error(
        "memory file was written and its generated MEMORY.md manifest could not be refreshed; rollback also failed: {rollback}"
    )]
    ManifestRefreshRollback {
        source: V2StorageError,
        rollback: Box<V2AccessError>,
    },
}

pub type V2AccessResult<T> = std::result::Result<T, V2AccessError>;

/// Shared per-session access policy and read-snapshot tracker.
#[derive(Debug)]
pub struct V2MemoryAccessPolicy {
    global_root: PathBuf,
    global_canonical_root: PathBuf,
    workspace_root: PathBuf,
    workspace_canonical_root: PathBuf,
    snapshots: parking_lot::Mutex<HashMap<PathBuf, blake3::Hash>>,
    write_lock: parking_lot::Mutex<()>,
    /// `(configured, canonical)` project-note directory, when one exists.
    project_root: Option<(PathBuf, PathBuf)>,
}

struct ValidatedV2Write {
    class: V2PathClass,
    canonical: PathBuf,
    previous_content: Option<Vec<u8>>,
}

impl V2MemoryAccessPolicy {
    /// Build a policy over already initialized scope roots.
    ///
    /// # Errors
    ///
    /// Returns [`V2AccessError::Inspect`] if either root cannot be canonicalized.
    pub fn new(global_root: &Path, workspace_root: &Path) -> V2AccessResult<Self> {
        Ok(Self {
            global_root: global_root.to_path_buf(),
            global_canonical_root: canonicalize_root(global_root)?,
            workspace_root: workspace_root.to_path_buf(),
            workspace_canonical_root: canonicalize_root(workspace_root)?,
            snapshots: parking_lot::Mutex::new(HashMap::new()),
            write_lock: parking_lot::Mutex::new(()),
            project_root: None,
        })
    }

    /// Also govern the curated project note in `project_dir`.
    ///
    /// # Errors
    ///
    /// Returns [`V2AccessError::Inspect`] if the directory cannot be canonicalized.
    pub fn with_project_dir(mut self, project_dir: &Path) -> V2AccessResult<Self> {
        self.project_root = Some((project_dir.to_path_buf(), canonicalize_root(project_dir)?));
        Ok(self)
    }

    /// The project note path when `path` targets the project directory.
    /// Anything in that directory except `PROJECT.md` is protected.
    fn project_target(&self, path: &Path) -> V2AccessResult<Option<PathBuf>> {
        let Some((root, canonical_root)) = &self.project_root else {
            return Ok(None);
        };
        if !path.is_absolute() {
            return Ok(None);
        }
        let inside = path.starts_with(root)
            || resolve_nearest_existing(path)?.starts_with(canonical_root);
        if !inside {
            return Ok(None);
        }
        if path.starts_with(root) {
            reject_symlink_components(root, path)?;
        }
        let target = canonical_root.join(crate::v2_project::PROJECT_NOTE_FILE);
        let matches = path == root.join(crate::v2_project::PROJECT_NOTE_FILE)
            || resolve_nearest_existing(path)? == target;
        if !matches {
            return Err(V2AccessError::Protected(path.to_path_buf()));
        }
        Ok(Some(target))
    }

    fn validate_project_write(
        &self,
        path: &Path,
        canonical: &Path,
        contents: &[u8],
    ) -> V2AccessResult<Option<Vec<u8>>> {
        let chars = String::from_utf8_lossy(contents).chars().count();
        if chars > crate::v2_project::PROJECT_NOTE_MAX_CHARS {
            return Err(V2AccessError::ProjectNoteTooLong {
                path: path.to_path_buf(),
                limit: crate::v2_project::PROJECT_NOTE_MAX_CHARS,
                chars,
            });
        }
        if let Some(kind) = crate::secret_guard::find_secret_bytes(contents) {
            return Err(V2AccessError::Secret {
                path: path.to_path_buf(),
                kind,
            });
        }
        match read_previous_content(canonical) {
            Ok(bytes) => {
                let snapshots = self.snapshots.lock();
                let Some(expected) = snapshots.get(canonical) else {
                    return Err(V2AccessError::ReadRequired(canonical.to_path_buf()));
                };
                if *expected != blake3::hash(&bytes) {
                    return Err(V2AccessError::Stale(canonical.to_path_buf()));
                }
                Ok(Some(bytes))
            }
            Err(V2AccessError::Inspect { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn write_project_note(
        &self,
        path: &Path,
        canonical: &Path,
        contents: &[u8],
    ) -> V2AccessResult<MemoryV2Write> {
        let _write_guard = self.write_lock.lock();
        let previous_content = self.validate_project_write(path, canonical, contents)?;
        persist_atomically(canonical, contents, previous_content.is_none())?;
        crate::secret_guard::restrict_file(canonical);
        self.snapshots
            .lock()
            .insert(canonical.to_path_buf(), blake3::hash(contents));
        Ok(MemoryV2Write::Written { previous_content })
    }

    /// Refuse a brand-new topic whose words nearly match an existing topic.
    fn check_new_topic_duplicate(&self, path: &Path, contents: &[u8]) -> V2AccessResult<()> {
        let V2PathClass::Topic(scope) = self.classify_path(path)? else {
            return Ok(());
        };
        if std::fs::symlink_metadata(path).is_ok() {
            return Ok(());
        }
        let words = crate::v2_dedupe::note_word_set(&String::from_utf8_lossy(contents));
        if words.len() < 4 {
            return Ok(());
        }
        let topics_dir = self.root_for_scope(scope).join("topics");
        let Ok(entries) = std::fs::read_dir(&topics_dir) else {
            return Ok(());
        };
        for entry in entries.filter_map(Result::ok).take(512) {
            let existing = entry.path();
            if existing.extension().is_none_or(|ext| ext != "md")
                || entry.metadata().map_or(true, |meta| !meta.is_file() || meta.len() > MAX_TOPIC_FILE_BYTES)
            {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&existing) else {
                continue;
            };
            let other = crate::v2_dedupe::note_word_set(&text);
            if other.len() >= 4 && crate::v2_dedupe::jaccard(&words, &other) >= 0.8 {
                return Err(V2AccessError::NearDuplicate {
                    path: path.to_path_buf(),
                    existing,
                });
            }
        }
        Ok(())
    }

    fn record_topic_created(&self, path: &Path) {
        let Ok(V2PathClass::Topic(scope)) = self.classify_path(path) else {
            return;
        };
        let root = self.root_for_scope(scope);
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        let mut ledger = crate::v2_usage::V2UsageLedger::default();
        let now = crate::v2_clock::V2Clock::now_unix_seconds(&crate::v2_clock::SystemV2Clock);
        ledger.ensure_created(&format!("topics/{name}"), now);
        if let Err(error) = ledger.save_merged(root, &[]) {
            tracing::debug!(%error, "could not record memory note creation");
        }
    }

    /// Classify a path against both configured and resolved scope roots.
    ///
    /// # Errors
    ///
    /// Returns [`V2AccessError::Inspect`] if the nearest existing path prefix
    /// cannot be resolved.
    pub fn classify_path(&self, path: &Path) -> V2AccessResult<V2PathClass> {
        if !path.is_absolute() {
            return Err(V2AccessError::RelativePath(path.to_path_buf()));
        }
        let configured_class = [
            (&self.workspace_root, V2MemoryScope::Workspace),
            (&self.global_root, V2MemoryScope::Global),
        ]
        .into_iter()
        .find_map(|(root, scope)| classify_against_root(path, root, scope));
        let configured_scope = [
            (&self.workspace_root, V2MemoryScope::Workspace),
            (&self.global_root, V2MemoryScope::Global),
        ]
        .into_iter()
        .find_map(|(root, scope)| path.strip_prefix(root).ok().map(|_| scope));
        let resolved = resolve_nearest_existing(path)?;
        let resolved_class = [
            (&self.workspace_canonical_root, V2MemoryScope::Workspace),
            (&self.global_canonical_root, V2MemoryScope::Global),
        ]
        .into_iter()
        .find_map(|(root, scope)| classify_against_root(&resolved, root, scope));

        if let Some(resolved_class) = resolved_class {
            if configured_class.is_none()
                && let Some(scope) = configured_scope
            {
                return Ok(if matches!(resolved_class, V2PathClass::Manifest(_)) {
                    resolved_class
                } else {
                    V2PathClass::Protected(scope)
                });
            }
            if configured_class
                .and_then(V2PathClass::scope)
                .is_some_and(|scope| Some(scope) != resolved_class.scope())
            {
                return Err(V2AccessError::EscapesScope(path.to_path_buf()));
            }
            return Ok(resolved_class);
        }
        Ok(configured_class.unwrap_or(V2PathClass::Outside))
    }

    /// Remove a previously read topic file with the same containment and stale
    /// snapshot checks used by ordinary v2 writes.
    ///
    /// # Errors
    ///
    /// Returns [`V2AccessError::Protected`] unless `path` is a topic Markdown
    /// file, or [`V2AccessError::ReadRequired`] / [`V2AccessError::Stale`] when
    /// the caller has not observed the current bytes.
    pub fn remove_topic_file(&self, path: &Path) -> V2AccessResult<()> {
        self.remove_topic_file_inner(path, None)
    }

    pub(crate) fn remove_topic_file_in_transaction(
        &self,
        path: &Path,
        transaction: &rusqlite::Transaction<'_>,
    ) -> V2AccessResult<()> {
        self.remove_topic_file_inner(path, Some(transaction))
    }

    fn remove_topic_file_inner(
        &self,
        path: &Path,
        transaction: Option<&rusqlite::Transaction<'_>>,
    ) -> V2AccessResult<()> {
        let _write_guard = self.write_lock.lock();
        let class = self.classify_path(path)?;
        if matches!(class, V2PathClass::Nested(_)) {
            return Err(V2AccessError::NestedPath(path.to_path_buf()));
        }
        if !matches!(class, V2PathClass::Topic(_)) {
            return Err(V2AccessError::Protected(path.to_path_buf()));
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
            return Err(V2AccessError::NonMarkdown(path.to_path_buf()));
        }
        let canonical = self.validate_containment(path, class)?;
        let metadata =
            std::fs::symlink_metadata(&canonical).map_err(|source| V2AccessError::Inspect {
                path: canonical.clone(),
                source,
            })?;
        if !metadata.file_type().is_file() || metadata.len() > MAX_TOPIC_FILE_BYTES {
            return Err(V2AccessError::Inspect {
                path: canonical.clone(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "topic is not a bounded regular file",
                ),
            });
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        std::fs::File::open(&canonical)
            .and_then(|mut file| {
                std::io::Read::by_ref(&mut file)
                    .take(MAX_TOPIC_FILE_BYTES + 1)
                    .read_to_end(&mut bytes)
            })
            .map_err(|source| V2AccessError::Inspect {
                path: canonical.clone(),
                source,
            })?;
        if bytes.len() as u64 > MAX_TOPIC_FILE_BYTES {
            return Err(V2AccessError::Inspect {
                path: canonical.clone(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "topic exceeds its read limit",
                ),
            });
        }
        let observed_hash = blake3::hash(&bytes);
        let snapshots = self.snapshots.lock();
        let Some(expected) = snapshots.get(&canonical) else {
            return Err(V2AccessError::ReadRequired(canonical));
        };
        if *expected != observed_hash {
            return Err(V2AccessError::Stale(canonical));
        }
        drop(snapshots);
        std::fs::remove_file(&canonical).map_err(|source| V2AccessError::Write {
            path: canonical.clone(),
            source,
        })?;
        self.snapshots.lock().remove(&canonical);
        let scope = class
            .scope()
            .ok_or_else(|| V2AccessError::EscapesScope(path.to_path_buf()))?;
        self.refresh_manifest(scope, transaction)
            .map_err(|source| {
                // The manifest makes deletion observable. Restore the topic on a refresh
                // failure so the caller can retry from the same snapshot.
                match persist_atomically(&canonical, &bytes, true) {
                    Ok(()) => {
                        self.snapshots
                            .lock()
                            .insert(canonical.clone(), observed_hash);
                        V2AccessError::ManifestRefresh { source }
                    }
                    Err(rollback) => V2AccessError::ManifestRefreshRollback {
                        source,
                        rollback: Box::new(rollback),
                    },
                }
            })?;
        Ok(())
    }

    pub(crate) fn validate_read_typed(&self, path: &Path) -> V2AccessResult<bool> {
        self.validate_read_inner(path)
    }

    pub(crate) fn record_read_typed(&self, path: &Path, contents: &[u8]) -> V2AccessResult<()> {
        self.record_read_inner(path, contents)
    }

    pub(crate) fn write_file_typed_in_transaction(
        &self,
        path: &Path,
        contents: &[u8],
        transaction: &rusqlite::Transaction<'_>,
    ) -> V2AccessResult<MemoryV2Write> {
        self.write_file_inner_with_transaction(path, contents, Some(transaction))
    }

    fn root_for_scope(&self, scope: V2MemoryScope) -> &Path {
        match scope {
            V2MemoryScope::Global => &self.global_root,
            V2MemoryScope::Workspace => &self.workspace_root,
        }
    }

    fn canonical_root_for_scope(&self, scope: V2MemoryScope) -> &Path {
        match scope {
            V2MemoryScope::Global => &self.global_canonical_root,
            V2MemoryScope::Workspace => &self.workspace_canonical_root,
        }
    }

    fn refresh_manifest(
        &self,
        scope: V2MemoryScope,
        transaction: Option<&rusqlite::Transaction<'_>>,
    ) -> std::result::Result<(), V2StorageError> {
        let root = self.root_for_scope(scope);
        match transaction {
            Some(transaction) => bump_manifest_revision_in_transaction(root, transaction),
            None => bump_manifest_revision(root),
        }
        .and_then(|()| {
            regenerate_scope_manifest(root, scope, V2ManifestBudget::default()).map(|_| ())
        })
    }

    fn validate_containment(&self, path: &Path, class: V2PathClass) -> V2AccessResult<PathBuf> {
        let Some(scope) = class.scope() else {
            return Ok(path.to_path_buf());
        };
        let root = self.root_for_scope(scope);
        let canonical_root = self.canonical_root_for_scope(scope);
        if path.starts_with(root) {
            reject_symlink_components(root, path)?;
        } else {
            let common = common_ancestor(root, path);
            reject_symlink_components(&common, path)?;
        }

        let canonical = resolve_nearest_existing(path)?;
        if !canonical.starts_with(canonical_root) {
            return Err(V2AccessError::EscapesScope(path.to_path_buf()));
        }
        Ok(canonical)
    }

    fn validate_read_inner(&self, path: &Path) -> V2AccessResult<bool> {
        let class = self.classify_path(path)?;
        if class == V2PathClass::Outside {
            return Ok(false);
        }
        let canonical = self.validate_containment(path, class)?;
        let scope = class
            .scope()
            .ok_or_else(|| V2AccessError::EscapesScope(path.to_path_buf()))?;
        let relative = canonical
            .strip_prefix(self.canonical_root_for_scope(scope))
            .map_err(|_| V2AccessError::EscapesScope(path.to_path_buf()))?;
        if is_durably_excluded(self.root_for_scope(scope), relative).map_err(|source| {
            V2AccessError::ExclusionState {
                path: path.to_path_buf(),
                source,
            }
        })? {
            return Err(V2AccessError::Excluded(path.to_path_buf()));
        }
        Ok(true)
    }

    fn record_read_inner(&self, path: &Path, contents: &[u8]) -> V2AccessResult<()> {
        let class = self.classify_path(path)?;
        if class != V2PathClass::Outside {
            let canonical = self.validate_containment(path, class)?;
            self.snapshots
                .lock()
                .insert(canonical, blake3::hash(contents));
        }
        Ok(())
    }

    fn validate_write_locked(
        &self,
        path: &Path,
        contents: &[u8],
    ) -> V2AccessResult<Option<ValidatedV2Write>> {
        let class = self.classify_path(path)?;
        if class == V2PathClass::Outside {
            return Ok(None);
        }
        if matches!(class, V2PathClass::Nested(_)) {
            return Err(V2AccessError::NestedPath(path.to_path_buf()));
        }
        if !class.is_writable() {
            return Err(V2AccessError::Protected(path.to_path_buf()));
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
            return Err(V2AccessError::NonMarkdown(path.to_path_buf()));
        }
        let limit_bytes = match class {
            V2PathClass::Observation(_) => MAX_WRITE_OBSERVATION_BYTES,
            _ => MAX_TOPIC_FILE_BYTES,
        };
        if contents.len() as u64 > limit_bytes {
            return Err(V2AccessError::TooLarge {
                path: path.to_path_buf(),
                limit_bytes,
            });
        }
        if let Some(kind) = crate::secret_guard::find_secret_bytes(contents) {
            return Err(V2AccessError::Secret {
                path: path.to_path_buf(),
                kind,
            });
        }

        let canonical = self.validate_containment(path, class)?;
        let scope = class
            .scope()
            .ok_or_else(|| V2AccessError::EscapesScope(path.to_path_buf()))?;
        let relative = canonical
            .strip_prefix(self.canonical_root_for_scope(scope))
            .map_err(|_| V2AccessError::EscapesScope(path.to_path_buf()))?;
        if is_durably_excluded(self.root_for_scope(scope), relative).map_err(|source| {
            V2AccessError::ExclusionState {
                path: path.to_path_buf(),
                source,
            }
        })? {
            return Err(V2AccessError::Excluded(path.to_path_buf()));
        }
        if matches!(class, V2PathClass::Observation(_))
            && std::fs::symlink_metadata(&canonical).is_ok()
        {
            return Err(V2AccessError::ImmutableObservation(path.to_path_buf()));
        }
        let previous_content = match read_previous_content(&canonical) {
            Ok(bytes) => {
                let snapshots = self.snapshots.lock();
                let Some(expected) = snapshots.get(&canonical) else {
                    return Err(V2AccessError::ReadRequired(canonical));
                };
                if *expected != blake3::hash(&bytes) {
                    return Err(V2AccessError::Stale(canonical));
                }
                Some(bytes)
            }
            Err(V2AccessError::Inspect { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(error) => return Err(error),
        };
        Ok(Some(ValidatedV2Write {
            class,
            canonical,
            previous_content,
        }))
    }

    fn preflight_write_inner(&self, path: &Path, contents: &[u8]) -> V2AccessResult<bool> {
        let _write_guard = self.write_lock.lock();
        self.validate_write_locked(path, contents)
            .map(|validated| validated.is_some())
    }

    fn write_file_inner(&self, path: &Path, contents: &[u8]) -> V2AccessResult<MemoryV2Write> {
        self.write_file_inner_with_transaction(path, contents, None)
    }

    fn write_file_inner_with_transaction(
        &self,
        path: &Path,
        contents: &[u8],
        transaction: Option<&rusqlite::Transaction<'_>>,
    ) -> V2AccessResult<MemoryV2Write> {
        let _write_guard = self.write_lock.lock();
        let Some(validated) = self.validate_write_locked(path, contents)? else {
            return Ok(MemoryV2Write::Outside);
        };
        let ValidatedV2Write {
            class,
            mut canonical,
            previous_content,
        } = validated;

        let parent = canonical
            .parent()
            .ok_or_else(|| V2AccessError::EscapesScope(path.to_path_buf()))?;
        std::fs::create_dir_all(parent).map_err(|source| V2AccessError::Write {
            path: path.to_path_buf(),
            source,
        })?;
        // Creating missing parents changes which prefix is canonicalized. Check
        // containment and symlink components again before opening the tempfile.
        canonical = self.validate_containment(path, class)?;
        persist_atomically(&canonical, contents, previous_content.is_none())?;

        let scope = class
            .scope()
            .ok_or_else(|| V2AccessError::EscapesScope(path.to_path_buf()))?;
        self.refresh_manifest(scope, transaction)
            .map_err(|source| {
                let rollback = if let Some(previous_content) = previous_content.as_deref() {
                    persist_atomically(&canonical, previous_content, false)
                } else {
                    std::fs::remove_file(&canonical).map_err(|rollback_source| {
                        V2AccessError::Write {
                            path: canonical.clone(),
                            source: rollback_source,
                        }
                    })
                };
                match rollback {
                    Ok(()) => V2AccessError::ManifestRefresh { source },
                    Err(rollback) => V2AccessError::ManifestRefreshRollback {
                        source,
                        rollback: Box::new(rollback),
                    },
                }
            })?;
        self.snapshots
            .lock()
            .insert(canonical, blake3::hash(contents));
        Ok(MemoryV2Write::Written { previous_content })
    }
}

impl MemoryV2Access for V2MemoryAccessPolicy {
    fn validate_read(&self, path: &Path) -> Result<bool, String> {
        match self.project_target(path) {
            Ok(Some(_)) => return Ok(true),
            Ok(None) => {}
            Err(error) => return Err(error.to_string()),
        }
        self.validate_read_inner(path)
            .map_err(|error| error.to_string())
    }

    fn record_read(&self, path: &Path, contents: &[u8]) -> Result<(), String> {
        match self.project_target(path) {
            Ok(Some(canonical)) => {
                self.snapshots
                    .lock()
                    .insert(canonical, blake3::hash(contents));
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => return Err(error.to_string()),
        }
        self.record_read_inner(path, contents)
            .map_err(|error| error.to_string())
    }

    fn preflight_write(&self, path: &Path, contents: &[u8]) -> Result<bool, String> {
        (|| {
            if let Some(canonical) = self.project_target(path)? {
                let _write_guard = self.write_lock.lock();
                self.validate_project_write(path, &canonical, contents)?;
                return Ok(true);
            }
            self.check_new_topic_duplicate(path, contents)?;
            self.preflight_write_inner(path, contents)
        })()
        .map_err(|error: V2AccessError| error.to_string())
    }

    fn write_file(&self, path: &Path, contents: &[u8]) -> Result<MemoryV2Write, String> {
        (|| {
            if let Some(canonical) = self.project_target(path)? {
                return self.write_project_note(path, &canonical, contents);
            }
            self.check_new_topic_duplicate(path, contents)?;
            let written = self.write_file_inner(path, contents)?;
            if matches!(
                written,
                MemoryV2Write::Written {
                    previous_content: None
                }
            ) {
                self.record_topic_created(path);
            }
            Ok(written)
        })()
        .map_err(|error: V2AccessError| error.to_string())
    }

    fn scope_roots(&self) -> [PathBuf; 2] {
        [self.global_root.clone(), self.workspace_root.clone()]
    }
}

fn canonicalize_root(root: &Path) -> V2AccessResult<PathBuf> {
    dunce::canonicalize(root).map_err(|source| V2AccessError::Inspect {
        path: root.to_path_buf(),
        source,
    })
}

fn classify_against_root(path: &Path, root: &Path, scope: V2MemoryScope) -> Option<V2PathClass> {
    let relative = path.strip_prefix(root).ok()?;
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return None;
    }
    if relative == Path::new("MEMORY.md") {
        return Some(V2PathClass::Manifest(scope));
    }
    if let Ok(below) = relative.strip_prefix(Path::new("topics")) {
        return Some(if below.components().count() > 1 {
            V2PathClass::Nested(scope)
        } else {
            V2PathClass::Topic(scope)
        });
    }
    if let Ok(below) = relative.strip_prefix(Path::new("observations").join("_inbox")) {
        return Some(if below.components().count() > 1 {
            V2PathClass::Nested(scope)
        } else {
            V2PathClass::Observation(scope)
        });
    }
    if relative.starts_with(Path::new("archive")) {
        return Some(V2PathClass::ArchivedObservation(scope));
    }
    Some(V2PathClass::Protected(scope))
}

fn resolve_nearest_existing(path: &Path) -> V2AccessResult<PathBuf> {
    for ancestor in path.ancestors() {
        match dunce::canonicalize(ancestor) {
            Ok(canonical_ancestor) => {
                let relative = path
                    .strip_prefix(ancestor)
                    .map_err(|_| V2AccessError::EscapesScope(path.to_path_buf()))?;
                return normalize_path(&canonical_ancestor.join(relative), path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(V2AccessError::Inspect {
                    path: ancestor.to_path_buf(),
                    source,
                });
            }
        }
    }
    Err(V2AccessError::EscapesScope(path.to_path_buf()))
}

fn normalize_path(candidate: &Path, original: &Path) -> V2AccessResult<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(V2AccessError::Traversal(original.to_path_buf()));
                }
            }
        }
    }
    Ok(normalized)
}

fn common_ancestor(left: &Path, right: &Path) -> PathBuf {
    let mut common = PathBuf::new();
    for (left_component, right_component) in left.components().zip(right.components()) {
        if left_component != right_component {
            break;
        }
        common.push(left_component.as_os_str());
    }
    common
}

fn reject_symlink_components(root: &Path, path: &Path) -> V2AccessResult<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| V2AccessError::EscapesScope(path.to_path_buf()))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(V2AccessError::Symlink(current));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(source) => {
                return Err(V2AccessError::Inspect {
                    path: current,
                    source,
                });
            }
        }
    }
    Ok(())
}

fn read_previous_content(path: &Path) -> V2AccessResult<Vec<u8>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|source| V2AccessError::Inspect {
            path: path.to_path_buf(),
            source,
        })?;
    let metadata = file.metadata().map_err(|source| V2AccessError::Inspect {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(V2AccessError::Inspect {
            path: path.to_path_buf(),
            source: std::io::Error::other("memory v2 path is not a regular file"),
        });
    }
    if metadata.len() > MAX_PREVIOUS_CONTENT_BYTES {
        return Err(V2AccessError::TooLarge {
            path: path.to_path_buf(),
            limit_bytes: MAX_PREVIOUS_CONTENT_BYTES,
        });
    }

    let mut bytes = Vec::new();
    file.take(MAX_PREVIOUS_CONTENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| V2AccessError::Inspect {
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() as u64 > MAX_PREVIOUS_CONTENT_BYTES {
        return Err(V2AccessError::TooLarge {
            path: path.to_path_buf(),
            limit_bytes: MAX_PREVIOUS_CONTENT_BYTES,
        });
    }
    Ok(bytes)
}

fn persist_atomically(path: &Path, contents: &[u8], is_new: bool) -> V2AccessResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| V2AccessError::EscapesScope(path.to_path_buf()))?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| V2AccessError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    temporary
        .write_all(contents)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| V2AccessError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    let persisted = if is_new {
        temporary.persist_noclobber(path)
    } else {
        temporary.persist(path)
    };
    persisted.map(|_| ()).map_err(|error| V2AccessError::Write {
        path: path.to_path_buf(),
        source: error.error,
    })
}

#[cfg(test)]
#[path = "v2_access_tests.rs"]
mod tests;
