//! Build-status, template-tag, and builder data types for the template builder.
//!
//! These types wrap the generated API schema types (which remain
//! `pub(crate)`) and expose a stable, ergonomic public interface. All
//! constructors go through crate-internal `from_wire` associated functions that
//! accept the generated types and perform any necessary conversions (e.g. UUID
//! to `String`, ANSI stripping via [`LogEntry`]).
//!
//! # Re-exports
//!
//! These types are re-exported at the crate root:
//! - [`BuildStatus`]
//! - [`BuildStatusReason`]
//! - [`TemplateTag`]
//! - [`TemplateBuildStatusResponse`]
//! - [`BuildInfo`]
//! - [`InstructionType`]
//! - [`Instruction`]
//! - [`CopyItem`]

use chrono::{DateTime, Utc};

use crate::template::log::LogEntry;

/// Current status of a template build.
///
/// Mirrors the wire `TemplateBuildStatus` enum with idiomatic Rust naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStatus {
    /// The build is currently in progress.
    Building,
    /// The build is queued and waiting to start.
    Waiting,
    /// The build completed successfully and the template is ready to use.
    Ready,
    /// The build failed. See [`TemplateBuildStatusResponse::reason`] for
    /// details.
    Error,
}

impl BuildStatus {
    /// Converts the generated [`crate::api::schema::TemplateBuildStatus`] into
    /// a public [`BuildStatus`].
    // Called from TemplateBuildStatusResponse::from_wire (Plan 5b callers arrive later).
    #[allow(dead_code)]
    pub(crate) fn from_wire(s: crate::api::schema::TemplateBuildStatus) -> Self {
        use crate::api::schema::TemplateBuildStatus as W;
        match s {
            W::Building => Self::Building,
            W::Waiting => Self::Waiting,
            W::Ready => Self::Ready,
            W::Error => Self::Error,
        }
    }
}

/// Human-readable explanation of why a build reached its current status.
///
/// Usually present when [`BuildStatus`] is [`BuildStatus::Error`].
#[derive(Debug, Clone)]
pub struct BuildStatusReason {
    /// Human-readable description of the status reason.
    pub message: String,
    /// Build step during which the reason occurred, if applicable.
    pub step: Option<String>,
    /// Structured log entries associated with this status reason. Messages
    /// have ANSI escape sequences stripped.
    pub log_entries: Vec<LogEntry>,
}

impl BuildStatusReason {
    /// Converts the generated [`crate::api::schema::BuildStatusReason`] into a
    /// public [`BuildStatusReason`].
    ///
    /// Each [`crate::api::schema::BuildLogEntry`] in `log_entries` is mapped
    /// via [`LogEntry::from_wire`], which strips ANSI sequences from messages.
    // Called from TemplateBuildStatusResponse::from_wire (Plan 5b callers arrive later).
    #[allow(dead_code)]
    pub(crate) fn from_wire(w: crate::api::schema::BuildStatusReason) -> Self {
        Self {
            message: w.message,
            step: w.step,
            log_entries: w.log_entries.into_iter().map(LogEntry::from_wire).collect(),
        }
    }
}

/// A named tag associated with a specific template build.
///
/// Tags allow builds to be referenced by a human-readable label rather than
/// a raw build UUID.
#[derive(Debug, Clone)]
pub struct TemplateTag {
    /// The tag name (e.g. `"latest"` or `"v1.2"`).
    pub tag: String,
    /// String representation of the UUID identifying the build this tag points
    /// to.
    pub build_id: String,
    /// When this tag was assigned.
    pub created_at: DateTime<Utc>,
}

impl TemplateTag {
    /// Converts the generated [`crate::api::schema::TemplateTag`] into a
    /// public [`TemplateTag`].
    ///
    /// The generated `build_id` field is a [`uuid::Uuid`]; it is exposed here
    /// as a `String` via `.to_string()`.
    // Callers arrive in Plan 5b/5c.
    #[allow(dead_code)]
    pub(crate) fn from_wire(w: crate::api::schema::TemplateTag) -> Self {
        Self {
            tag: w.tag,
            build_id: w.build_id.to_string(),
            created_at: w.created_at,
        }
    }
}

/// Combined status and structured logs for an in-progress or completed
/// template build.
///
/// Returned when polling the build status endpoint. Log entries are available
/// both as raw strings ([`logs`][TemplateBuildStatusResponse::logs]) and as
/// structured, ANSI-stripped entries
/// ([`log_entries`][TemplateBuildStatusResponse::log_entries]).
#[derive(Debug, Clone)]
pub struct TemplateBuildStatusResponse {
    /// Identifier of the template being built.
    pub template_id: String,
    /// Identifier of the specific build.
    pub build_id: String,
    /// Current build status.
    pub status: BuildStatus,
    /// Raw build log lines as plain strings.
    pub logs: Vec<String>,
    /// Structured build log entries (ANSI sequences stripped from messages).
    pub log_entries: Vec<LogEntry>,
    /// Reason for the current status; usually present when
    /// [`status`][TemplateBuildStatusResponse::status] is
    /// [`BuildStatus::Error`].
    pub reason: Option<BuildStatusReason>,
}

impl TemplateBuildStatusResponse {
    /// Converts the generated [`crate::api::schema::TemplateBuildInfo`] into a
    /// public [`TemplateBuildStatusResponse`].
    ///
    /// Each [`crate::api::schema::BuildLogEntry`] in `log_entries` is mapped
    /// via [`LogEntry::from_wire`].
    // Callers arrive in Plan 5b/5c.
    #[allow(dead_code)]
    pub(crate) fn from_wire(w: crate::api::schema::TemplateBuildInfo) -> Self {
        Self {
            template_id: w.template_id,
            build_id: w.build_id,
            status: BuildStatus::from_wire(w.status),
            logs: w.logs,
            log_entries: w.log_entries.into_iter().map(LogEntry::from_wire).collect(),
            reason: w.reason.map(BuildStatusReason::from_wire),
        }
    }
}

/// The type of a single instruction in the template build pipeline.
///
/// Mirrors the JavaScript SDK's `InstructionType` enum
/// (`COPY` / `ENV` / `RUN` / `WORKDIR` / `USER`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstructionType {
    /// Copy files or directories into the image (`COPY`).
    Copy,
    /// Set an environment variable (`ENV`).
    Env,
    /// Run a shell command (`RUN`).
    Run,
    /// Change the working directory (`WORKDIR`).
    Workdir,
    /// Change the current user (`USER`).
    User,
}

/// A single step in the template build pipeline.
///
/// Plans 5c/5d construct these and serialize them into the internal
/// `TemplateStep` wire type. This struct is the internal-but-public
/// representation used by builder methods.
#[derive(Debug, Clone)]
pub struct Instruction {
    /// The kind of this build instruction.
    pub instruction_type: InstructionType,
    /// Arguments passed to the instruction (e.g. source and destination paths,
    /// command strings, or key=value pairs).
    pub args: Vec<String>,
    /// If `true`, this instruction bypasses the build cache and is always
    /// re-executed.
    pub force: bool,
    /// If `true`, forces the file-upload step for `COPY` instructions even
    /// when the content hash matches.
    pub force_upload: Option<bool>,
    /// Content hash of the files involved in this instruction. Used by the
    /// build cache to detect changes.
    pub files_hash: Option<String>,
    /// Whether symbolic links in source paths should be resolved to their
    /// targets before copying.
    pub resolve_symlinks: bool,
}

/// Configuration for a single file or directory copy operation.
///
/// Mirrors the JavaScript SDK's `CopyItem` type. Collected by
/// [`crate::template`] builder methods and converted to [`Instruction`]
/// entries during build preparation (Plan 5c).
#[derive(Debug, Clone, Default)]
pub struct CopyItem {
    /// Source paths to copy. In the JS SDK, `src` accepts a single `PathLike`
    /// or an array; here it is always a `Vec<String>`.
    pub src: Vec<String>,
    /// Destination path inside the template image.
    pub dest: String,
    /// If `true`, forces the file upload even when the content hash matches.
    pub force_upload: Option<bool>,
    /// User (and optionally group) for the copied files, e.g. `"user:group"`.
    pub user: Option<String>,
    /// Unix file permission bits for the copied files (e.g. `0o755`).
    pub mode: Option<u32>,
    /// Whether to resolve symbolic links in source paths before copying.
    pub resolve_symlinks: bool,
}

/// Result of triggering a template build.
///
/// Returned after the build-trigger HTTP call succeeds (Plan 5c). Fields are
/// mapped from the internal `TemplateRequestResponseV3` wire type.
///
/// # Field mapping
///
/// | `BuildInfo` field | Wire source | Note |
/// |---|---|---|
/// | `template_id` | `templateID` | direct |
/// | `build_id` | `buildID` | direct |
/// | `name` | `names[0]` | first entry; `None` if the array is empty |
/// | `alias` | `aliases[0]` | first entry; `None` if the array is empty; **deprecated** |
/// | `tags` | `tags` | direct |
#[derive(Debug, Clone)]
pub struct BuildInfo {
    /// Identifier of the template.
    pub template_id: String,
    /// Identifier of this specific build.
    pub build_id: String,
    /// Name of the template (first entry from the `names` array returned by
    /// the API). `None` when the API returns an empty `names` array.
    pub name: Option<String>,
    /// Deprecated alias (first entry from the `aliases` array). Present for
    /// backward compatibility with the JS SDK's `BuildInfo.alias` field.
    /// Prefer [`BuildInfo::name`].
    pub alias: Option<String>,
    /// Tags assigned to this build (e.g. `["latest", "v1"]`).
    pub tags: Vec<String>,
}

impl BuildInfo {
    /// Converts the generated [`crate::api::schema::TemplateRequestResponseV3`]
    /// into a public [`BuildInfo`].
    ///
    /// - `name` is `names.first().cloned()` — `None` when the array is empty.
    /// - `alias` is `aliases.first().cloned()` — `None` when the array is
    ///   empty (the JS `alias` field is also the first entry and is deprecated).
    // Caller arrives in Plan 5c; suppress dead-code lint until then.
    #[allow(dead_code)]
    pub(crate) fn from_wire(w: crate::api::schema::TemplateRequestResponseV3) -> Self {
        Self {
            template_id: w.template_id,
            build_id: w.build_id,
            name: w.names.into_iter().next(),
            alias: w.aliases.into_iter().next(),
            tags: w.tags,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::TemplateBuildStatus as WireStatus;
    use crate::template::log::LogEntryLevel;

    #[test]
    fn build_status_from_wire() {
        assert_eq!(
            BuildStatus::from_wire(WireStatus::Building),
            BuildStatus::Building
        );
        assert_eq!(
            BuildStatus::from_wire(WireStatus::Waiting),
            BuildStatus::Waiting
        );
        assert_eq!(
            BuildStatus::from_wire(WireStatus::Ready),
            BuildStatus::Ready
        );
        assert_eq!(
            BuildStatus::from_wire(WireStatus::Error),
            BuildStatus::Error
        );
    }

    #[test]
    fn tag_from_wire() {
        let uuid = uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000")
            .expect("valid uuid literal");
        let wire = crate::api::schema::TemplateTag {
            build_id: uuid,
            created_at: "2024-06-01T00:00:00Z"
                .parse()
                .expect("valid timestamp literal"),
            tag: "latest".to_string(),
        };
        let tag = TemplateTag::from_wire(wire);
        assert_eq!(tag.tag, "latest");
        assert_eq!(tag.build_id, "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn serde_round_trip_build_info() {
        // Honest wire JSON: lowercase status, camelCase keys, logEntries with
        // lowercase level strings — matching the real API response format.
        let json = r#"{
            "templateID": "tmpl-abc123",
            "buildID": "build-xyz789",
            "status": "building",
            "logs": [],
            "logEntries": [
                {
                    "level": "info",
                    "message": "x",
                    "timestamp": "2024-01-01T00:00:00Z"
                }
            ]
        }"#;

        let wire: crate::api::schema::TemplateBuildInfo =
            serde_json::from_str(json).expect("valid TemplateBuildInfo JSON");
        let response = TemplateBuildStatusResponse::from_wire(wire);

        assert_eq!(response.status, BuildStatus::Building);
        assert_eq!(response.template_id, "tmpl-abc123");
        assert_eq!(response.build_id, "build-xyz789");
        assert_eq!(response.log_entries.len(), 1);
        assert_eq!(response.log_entries[0].level(), LogEntryLevel::Info);
        assert_eq!(response.log_entries[0].message(), "x");
    }

    #[test]
    fn serde_round_trip_with_reason() {
        let json = r#"{
            "templateID": "tmpl-err",
            "buildID": "build-err",
            "status": "error",
            "logs": ["something went wrong"],
            "logEntries": [],
            "reason": {
                "message": "Dockerfile syntax error",
                "step": "build",
                "logEntries": []
            }
        }"#;

        let wire: crate::api::schema::TemplateBuildInfo =
            serde_json::from_str(json).expect("valid TemplateBuildInfo JSON with reason");
        let response = TemplateBuildStatusResponse::from_wire(wire);

        assert_eq!(response.status, BuildStatus::Error);
        let reason = response.reason.expect("reason should be present");
        assert_eq!(reason.message, "Dockerfile syntax error");
        assert_eq!(reason.step.as_deref(), Some("build"));
    }

    #[test]
    fn instruction_type_roundtrip() {
        let ty = InstructionType::Copy;
        assert_eq!(ty, InstructionType::Copy);
        assert_ne!(ty, InstructionType::Run);
        // Copy trait: can use after being copied
        let ty2 = ty;
        assert_eq!(ty, ty2);
        // All variants are distinct
        assert_ne!(InstructionType::Env, InstructionType::Workdir);
        assert_ne!(InstructionType::User, InstructionType::Run);
    }

    #[test]
    fn copy_item_defaults() {
        let item = CopyItem::default();
        assert!(item.src.is_empty());
        assert_eq!(item.dest, "");
        assert_eq!(item.force_upload, None);
        assert_eq!(item.user, None);
        assert_eq!(item.mode, None);
        assert!(!item.resolve_symlinks);
    }

    #[test]
    fn build_info_from_wire() {
        // Honest camelCase wire JSON matching TemplateRequestResponseV3 schema.
        let json = r#"{
            "aliases": ["my-alias"],
            "buildID": "build-abc",
            "names": ["my-template"],
            "public": false,
            "tags": ["latest", "v1"],
            "templateID": "tmpl-xyz"
        }"#;
        let wire: crate::api::schema::TemplateRequestResponseV3 =
            serde_json::from_str(json).expect("valid TemplateRequestResponseV3 JSON");
        let info = BuildInfo::from_wire(wire);
        assert_eq!(info.template_id, "tmpl-xyz");
        assert_eq!(info.build_id, "build-abc");
        assert_eq!(info.name.as_deref(), Some("my-template"));
        assert_eq!(info.alias.as_deref(), Some("my-alias"));
        assert_eq!(info.tags, vec!["latest", "v1"]);
    }

    #[test]
    fn build_info_from_wire_empty_arrays() {
        // When names/aliases are empty, name and alias should be None.
        let json = r#"{
            "aliases": [],
            "buildID": "build-001",
            "names": [],
            "public": true,
            "tags": [],
            "templateID": "tmpl-001"
        }"#;
        let wire: crate::api::schema::TemplateRequestResponseV3 =
            serde_json::from_str(json).expect("valid TemplateRequestResponseV3 JSON");
        let info = BuildInfo::from_wire(wire);
        assert_eq!(info.name, None);
        assert_eq!(info.alias, None);
        assert!(info.tags.is_empty());
    }
}
