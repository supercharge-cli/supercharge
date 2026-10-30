//! Memory listing and on/off toggling for the `/memory` modal, shared by the slash command
//! and the `_supercharge/memory/{list,toggle}` extension methods.

use std::sync::Arc;

use crate::extensions::memory::{MemoryListing, MemoryToggleResponse};
use crate::session::acp_session::SessionActor;

impl SessionActor {
    /// Files the `/memory` modal shows plus the enabled/capability flags that drive its notices.
    /// Errors when the store cannot be listed; callers must not render that as an empty store.
    pub(crate) fn memory_listing(&self) -> Result<MemoryListing, String> {
        let disabled_reason = self.memory.disabled_reason();
        // Disabled sessions list the configured store: the notes still exist on disk, and the
        // modal hides them behind the disabled notice until `t` turns memory back on.
        let live = self.memory.storage.borrow();
        let files = match live.as_ref().or(self.memory.configured_storage.as_ref()) {
            Some(storage) => {
                let paths = storage.list_memory_files().map_err(|e| {
                    tracing::warn!(
                        session_id = %self.session_info.id.0,
                        error = %e,
                        "failed to list memory files",
                    );
                    format!("Failed to list memory files: {e}")
                })?;
                paths
                    .into_iter()
                    .map(|path| {
                        let meta = match std::fs::metadata(&path) {
                            Ok(m) => Some(m),
                            Err(e) => {
                                tracing::debug!(
                                    path = %path.display(),
                                    error = %e,
                                    "skipping memory file with unreadable metadata",
                                );
                                None
                            }
                        };
                        let generated = storage.mode().is_v2()
                            && (path == storage.global_memory_file()
                                || path == storage.workspace_memory_file());
                        crate::extensions::notification::MemoryFileInfo {
                            source: storage.classify_source(&path).to_string(),
                            generated,
                            path: path.display().to_string(),
                            size_bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                            modified_epoch_secs: meta
                                .and_then(|m| m.modified().ok())
                                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                .map(|d| d.as_secs()),
                        }
                    })
                    .collect()
            }
            None => vec![],
        };
        drop(live);
        // Legacy saves at compaction/session end when `save_on_end`; its `/dream` is always available.
        let v2 = &self.memory.v2_config;
        let (capture_enabled, dream_enabled) =
            if self.memory.mode() == Some(crate::config::MemoryMode::V2) {
                (v2.capture_enabled, v2.manual_dream_enabled)
            } else {
                (self.memory.save_on_end, true)
            };
        tracing::info!(
            session_id = %self.session_info.id.0,
            file_count = files.len(),
            enabled = disabled_reason.is_none(),
            ?disabled_reason,
            "memory browse: listing files",
        );
        Ok(MemoryListing {
            files,
            enabled: disabled_reason.is_none(),
            disabled_reason,
            capture_enabled,
            dream_enabled,
        })
    }

    /// Toggle, then attach a fresh listing so an open modal can resync.
    /// The toggle has already happened by the time listing runs, so a listing failure must not
    /// fail the reply; the modal keeps its rows and shows the toggle message.
    pub(crate) async fn memory_toggle_and_list(
        self: &Arc<Self>,
        enabled: bool,
    ) -> MemoryToggleResponse {
        let message = self.memory_toggle(enabled).await;
        let disabled_reason = self.memory.disabled_reason();
        MemoryToggleResponse {
            message,
            enabled: disabled_reason.is_none(),
            disabled_reason,
            listing: self.memory_listing().ok(),
        }
    }

    /// Turn memory on or off for this session and return the message for the user.
    pub(crate) async fn memory_toggle(self: &Arc<Self>, enabled: bool) -> String {
        tracing::info!(
            session_id = %self.session_info.id.0,
            enabled,
            "memory toggle",
        );
        use crate::extensions::notification::MemoryDisabledReason as Reason;
        let msg = if enabled && !self.memory.is_enabled() {
            match (
                self.memory.disabled_reason(),
                self.memory.configured_storage.clone(),
            ) {
                (Some(Reason::RolloutRestricted), _) => {
                    "Memory v2 cannot be enabled because this session's pinned rollout controls disable it."
                        .to_owned()
                }
                (Some(Reason::SessionToggle), Some(storage)) => {
                if let Err(e) =
                    crate::session::memory_state::initialize_memory_storage(storage.clone())
                        .await
                {
                    tracing::warn!(error = %e, "failed to initialize memory storage on re-enable");
                    format!("Memory could not be enabled: {e}")
                } else if self.memory.mode() == Some(crate::config::MemoryMode::V2) {
                    if self.memory.v2_config.can_expose_memory() {
                        let backend: std::sync::Arc<
                            dyn supercharge_tools::types::memory_backend::MemoryBackend,
                        > = std::sync::Arc::new(supercharge_memory::V2MemoryBackend::new(
                            storage.global_dir(),
                            storage.workspace_dir(),
                            self.memory.v2_recall.config.min_score,
                        ));
                        let bridge = self.agent.borrow().tool_bridge().clone();
                        bridge.update_resource(backend).await;
                        if let Err(e) = self.register_memory_tools(&bridge).await {
                            tracing::warn!(error = %e, "memory tool registration failed during toggle");
                        }
                    }
                    *self.memory.storage.borrow_mut() = Some(storage);
                    self.resume_v2_capture().await;
                    "Memory v2 enabled for this session.".to_owned()
                } else if let Some(ref params) = self.memory.backend_params {
                    let backend =
                        crate::session::memory::MemoryBackendImpl::from_session_params(
                            storage.clone(),
                            params,
                        );
                    *self.memory.search_counter.borrow_mut() =
                        Some(backend.search_counter.clone());
                    let backend: std::sync::Arc<
                        dyn supercharge_tools::types::memory_backend::MemoryBackend,
                    > = std::sync::Arc::new(backend);
                    let bridge = self.agent.borrow().tool_bridge().clone();
                    bridge.update_resource(backend.clone()).await;
                    if let Err(e) = self.register_memory_tools(&bridge).await {
                        tracing::warn!(error = %e, "memory tool registration failed during toggle");
                    }
                    *self.memory.storage.borrow_mut() = Some(storage);
                    "Memory enabled for this session.".to_owned()
                } else {
                    "Memory cannot be enabled (legacy backend not configured for this session)."
                        .to_owned()
                }
                }
                _ => {
                    "Memory cannot be enabled (not configured for this session).".to_owned()
                }
            }
        } else if !enabled && self.memory.is_enabled() {
            if self.memory.mode() == Some(crate::config::MemoryMode::V2) {
                self.memory.stop_capture_worker().await;
                self.memory.dream_workers.cancel_and_join().await;
            }
            let bridge = self.agent.borrow().tool_bridge().clone();
            if !bridge.unregister_tool_by_name(
                supercharge_tools::implementations::memory::MEMORY_SEARCH_TOOL_NAME,
            ) {
                tracing::debug!("memory_search tool was not registered during unregister");
            }
            if !bridge.unregister_tool_by_name(
                supercharge_tools::implementations::memory::MEMORY_GET_TOOL_NAME,
            ) {
                tracing::debug!("memory_get tool was not registered during unregister");
            }
            *self.memory.storage.borrow_mut() = None;
            *self.memory.search_counter.borrow_mut() = None;
            "Memory disabled for this session.".to_owned()
        } else {
            let state = if enabled { "enabled" } else { "disabled" };
            format!("Memory is already {state}.")
        };
        self.refresh_goal_harness_enabled().await;
        msg
    }
}
