//! Public filesystem value types (wrapping the generated `envd::proto` types).

use std::collections::BTreeMap;

use crate::envd::proto::filesystem as pb;

/// The kind of a filesystem entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    /// A regular file.
    File,
    /// A directory.
    Dir,
}

impl FileType {
    /// Map the generated proto enum (i32) to the public type; `None` for
    /// unspecified/unknown values (which the JS SDK filters out).
    // used by Task 2
    #[allow(dead_code)]
    pub(crate) fn from_proto(value: i32) -> Option<FileType> {
        match pb::FileType::try_from(value) {
            Ok(pb::FileType::File) => Some(FileType::File),
            Ok(pb::FileType::Directory) => Some(FileType::Dir),
            _ => None,
        }
    }
}

/// Metadata for a filesystem entry (`getInfo`, `list`, `rename`).
#[derive(Debug, Clone)]
pub struct EntryInfo {
    /// Base name of the entry.
    pub name: String,
    /// Absolute path of the entry.
    pub path: String,
    /// File or directory.
    pub r#type: FileType,
    /// Size in bytes.
    pub size: u64,
    /// Unix mode bits.
    pub mode: u32,
    /// Human-readable permission string (e.g. `-rw-r--r--`).
    pub permissions: String,
    /// Owning user.
    pub owner: String,
    /// Owning group.
    pub group: String,
    /// Last-modified time, if reported.
    pub modified_time: Option<chrono::DateTime<chrono::Utc>>,
    /// Target path if the entry is a symlink.
    pub symlink_target: Option<String>,
    /// User-defined `user.e2b.*` metadata (prefix stripped by envd).
    pub metadata: BTreeMap<String, String>,
}

impl EntryInfo {
    /// Map a generated proto entry to the public type. Returns `None` when the
    /// file type is unspecified/unknown (matching the JS SDK's filtering).
    // used by Task 2
    #[allow(dead_code)]
    pub(crate) fn from_proto(e: pb::EntryInfo) -> Option<EntryInfo> {
        let r#type = FileType::from_proto(e.r#type)?;
        let modified_time = e.modified_time.and_then(|t| {
            let nanos = u32::try_from(t.nanos).unwrap_or(0);
            chrono::DateTime::from_timestamp(t.seconds, nanos)
        });
        Some(EntryInfo {
            name: e.name,
            path: e.path,
            r#type,
            size: u64::try_from(e.size).unwrap_or(0),
            mode: e.mode,
            permissions: e.permissions,
            owner: e.owner,
            group: e.group,
            modified_time,
            symlink_target: e.symlink_target,
            metadata: e.metadata.into_iter().collect(),
        })
    }
}

/// Result of a write — a subset of [`EntryInfo`] returned by `POST /files`.
#[derive(Debug, Clone)]
pub struct WriteInfo {
    /// Base name of the written entry.
    pub name: String,
    /// Absolute path of the written entry.
    pub path: String,
    /// File or directory, if reported.
    pub r#type: Option<FileType>,
    /// Metadata persisted on the entry.
    pub metadata: BTreeMap<String, String>,
}

/// One entry in a multi-file batched upload (see [`crate::Filesystem`]).
#[derive(Debug, Clone)]
pub struct WriteEntry {
    /// Destination path in the sandbox.
    pub path: String,
    /// File contents.
    pub data: Vec<u8>,
}

/// The kind of change reported by a directory watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesystemEventType {
    /// An entry was created.
    Create,
    /// An entry was written.
    Write,
    /// An entry was removed.
    Remove,
    /// An entry was renamed.
    Rename,
    /// An entry's mode changed.
    Chmod,
}

impl FilesystemEventType {
    /// Map the generated proto enum (i32); `None` for unspecified/unknown.
    // used by Task 6
    #[allow(dead_code)]
    pub(crate) fn from_proto(value: i32) -> Option<FilesystemEventType> {
        match pb::EventType::try_from(value) {
            Ok(pb::EventType::Create) => Some(FilesystemEventType::Create),
            Ok(pb::EventType::Write) => Some(FilesystemEventType::Write),
            Ok(pb::EventType::Remove) => Some(FilesystemEventType::Remove),
            Ok(pb::EventType::Rename) => Some(FilesystemEventType::Rename),
            Ok(pb::EventType::Chmod) => Some(FilesystemEventType::Chmod),
            _ => None,
        }
    }
}

/// A single directory-watch event.
#[derive(Debug, Clone)]
pub struct FilesystemEvent {
    /// Path (relative to the watched dir) that changed.
    pub name: String,
    /// The kind of change.
    pub r#type: FilesystemEventType,
    /// Entry info, when `include_entry` was requested and the entry still exists.
    pub entry: Option<EntryInfo>,
}

impl FilesystemEvent {
    /// Map a generated proto event; `None` when the event type is unknown.
    // used by Task 6
    #[allow(dead_code)]
    pub(crate) fn from_proto(e: pb::FilesystemEvent) -> Option<FilesystemEvent> {
        let r#type = FilesystemEventType::from_proto(e.r#type)?;
        Some(FilesystemEvent {
            name: e.name,
            r#type,
            entry: e.entry.and_then(EntryInfo::from_proto),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envd::proto::filesystem as pb;

    #[test]
    fn entry_info_maps_file_and_filters_unknown_type() {
        let proto = pb::EntryInfo {
            name: "f.txt".into(),
            r#type: pb::FileType::File as i32,
            path: "/home/user/f.txt".into(),
            size: 12,
            mode: 0o644,
            permissions: "-rw-r--r--".into(),
            owner: "user".into(),
            group: "user".into(),
            modified_time: None,
            symlink_target: None,
            metadata: std::collections::HashMap::new(),
        };
        let e = EntryInfo::from_proto(proto).expect("file entry");
        assert_eq!(e.name, "f.txt");
        assert_eq!(e.r#type, FileType::File);
        assert_eq!(e.size, 12);

        let unknown = pb::EntryInfo {
            r#type: pb::FileType::Unspecified as i32,
            ..Default::default()
        };
        assert!(EntryInfo::from_proto(unknown).is_none());
    }

    #[test]
    fn event_maps_type() {
        let proto = pb::FilesystemEvent {
            name: "x".into(),
            r#type: pb::EventType::Write as i32,
            entry: None,
        };
        let ev = FilesystemEvent::from_proto(proto).expect("event");
        assert_eq!(ev.r#type, FilesystemEventType::Write);
    }
}
