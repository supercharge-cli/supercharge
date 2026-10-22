//! Per-turn memory archive upload: disabled.
//!
//! Memory notes are private local files (owner-only permissions). They are
//! never bundled into turn trace uploads; the manifest records the artifact
//! as skipped so turn-completeness accounting stays exact.

use super::turn::{PromptTraceContext, UploadWait};

/// Manifest skip reason for the always-skipped memory archive.
pub(crate) const MEMORY_UPLOAD_DISABLED_REASON: &str = "memory_upload_disabled";

/// Record the memory archive as skipped. Never reads or uploads memory files.
pub(crate) async fn upload_memory_state(ctx: &PromptTraceContext, _wait: UploadWait) {
    tracing::debug!("memory upload skipped: memory archives are never uploaded");
    super::manifest::skip_artifact(
        &ctx.artifact_tracker,
        "memory.tar.gz",
        MEMORY_UPLOAD_DISABLED_REASON,
    );
}
