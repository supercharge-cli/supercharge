//! Volume control-plane and content client, plus public types.
//!
//! The generated wire types in the internal `schema` module stay crate-private.
//! All user-facing types are re-exported here and again at the crate root.

pub(crate) mod schema;

pub(crate) mod client;

pub mod types;

pub(crate) mod control;

pub use types::{
    VolumeAndToken, VolumeEntryStat, VolumeFileType, VolumeInfo, VolumeListOpts, VolumeMakeDirOpts,
    VolumeMetadataOpts, VolumeReadOpts, VolumeWriteOpts,
};

pub use control::{Volume, VolumeOpts};

#[cfg(test)]
mod tests {
    use super::schema as volume_gen;

    #[test]
    fn volume_entry_stat_round_trips() {
        let json = r#"{
            "path": "/d/x.txt",
            "name": "x.txt",
            "size": 42,
            "mode": 420,
            "uid": 0,
            "gid": 0,
            "type": "file",
            "atime": "2023-11-14T22:13:20Z",
            "mtime": "2023-11-14T22:13:20Z",
            "ctime": "2023-11-14T22:13:20Z"
        }"#;
        let stat: volume_gen::VolumeEntryStat =
            serde_json::from_str(json).expect("deserialize VolumeEntryStat");
        assert_eq!(stat.name, "x.txt");
        assert_eq!(stat.size, 42);
        assert!(matches!(stat.type_, volume_gen::VolumeEntryStatType::File));
        // `type` field round-trips through the serde rename.
        let back = serde_json::to_value(&stat).expect("serialize");
        assert_eq!(back["type"], serde_json::json!("file"));
    }
}
