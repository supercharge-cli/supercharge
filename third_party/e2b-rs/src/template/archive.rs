//! Gzip-tar context archive for template builds.
//!
//! Ports `tarFileStream` from the E2B JS SDK
//! (`packages/js-sdk/src/template/utils.ts`, line 374).
//!
//! All items are `pub(crate)` — they are implementation details consumed by
//! the template builder and are not part of the public API.
//!
//! The function in this module is called by Task 3+ of Plan 5b (build
//! orchestration).  Until those callers land, the linter sees the item as
//! unused; the allow below suppresses that false positive.
#![allow(dead_code)]

use std::path::Path;

use crate::errors::{Error, Result};

// ─── tar_file_stream ──────────────────────────────────────────────────────────

/// Build a gzip-compressed tar archive of the files matching `src` within
/// `context` and spool it to a [`tempfile::NamedTempFile`].
///
/// Returns the temp-file handle (auto-deleted on drop) and the byte size of
/// the resulting archive.
///
/// ## Behaviour
///
/// 1. Calls [`crate::template::files::get_all_files_in_path`] to obtain a
///    sorted, deduplicated list of paths (including directory entries).
/// 2. Creates a [`tempfile::NamedTempFile`] as the write target.
/// 3. Wraps it with a [`flate2::write::GzEncoder`] (default compression) and
///    a [`tar::Builder`].
/// 4. Appends each path as a **single** entry using
///    `builder.append_path_with_name` — this adds the directory header or file
///    content without recursing into subdirectories (mirrors the JS SDK's
///    `noDirRecurse: true`).  The tar entry name is the context-relative POSIX
///    path (forward slashes).
/// 5. Finishes the encoder, stats the file for its byte length, and returns
///    `(named_temp_file, size)`.
///
/// ## Symlinks
///
/// `resolve_symlinks` is forwarded to [`tar::Builder::follow_symlinks`].
///
/// # Errors
///
/// Returns [`crate::Error::Internal`] on I/O or encoding failures, and
/// [`crate::Error::InvalidArgument`] for malformed glob/ignore patterns (from
/// the inner [`crate::template::files::get_all_files_in_path`] call).
pub(crate) fn tar_file_stream(
    src: &str,
    context: &Path,
    ignore: &[String],
    resolve_symlinks: bool,
) -> Result<(tempfile::NamedTempFile, u64)> {
    let files = crate::template::files::get_all_files_in_path(src, context, ignore)?;

    // Spool target — NamedTempFile is auto-deleted when dropped.
    let spool = tempfile::NamedTempFile::new()
        .map_err(|e| Error::Internal(format!("failed to create temp file: {e}")))?;

    // Clone the FD so the builder/encoder can own it while `spool` keeps the
    // NamedTempFile alive (and its delete-on-drop guard).
    let write_fd = spool
        .as_file()
        .try_clone()
        .map_err(|e| Error::Internal(format!("failed to clone temp-file fd: {e}")))?;

    let gz = flate2::write::GzEncoder::new(write_fd, flate2::Compression::default());
    let mut builder = tar::Builder::new(gz);
    builder.follow_symlinks(resolve_symlinks);

    for path in &files {
        // Context-relative POSIX entry name (forward slashes on all platforms).
        let entry_name = crate::template::files::relative_posix(path, context)?;

        // `append_path_with_name` adds a single entry (directory header or
        // file content) WITHOUT recursing — this mirrors `noDirRecurse: true`
        // in the JS SDK.  Each entry in `files` was already expanded by
        // `get_all_files_in_path`, so no further recursion is desired.
        builder
            .append_path_with_name(path, &entry_name)
            .map_err(|e| {
                Error::Internal(format!(
                    "failed to append \"{}\" to archive: {e}",
                    entry_name
                ))
            })?;
    }

    // Flush gzip stream and tar footer.
    let gz = builder
        .into_inner()
        .map_err(|e| Error::Internal(format!("failed to finalise tar builder: {e}")))?;
    gz.finish()
        .map_err(|e| Error::Internal(format!("failed to finalise gzip encoder: {e}")))?;

    // Stat through the original NamedTempFile handle — both FDs reference the
    // same file so the size is accurate after the encoder is flushed.
    let size = spool
        .as_file()
        .metadata()
        .map_err(|e| Error::Internal(format!("failed to stat archive: {e}")))?
        .len();

    Ok((spool, size))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::io::Read;

    /// Build a tiny tree (a.txt / sub/b.txt), tar it, read it back through a
    /// GzDecoder, and assert the entry names (relative POSIX) + contents match.
    #[test]
    fn tar_roundtrips_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        fs::write(root.join("a.txt"), b"hello").expect("write a.txt");
        fs::create_dir(root.join("sub")).expect("mkdir sub");
        fs::write(root.join("sub").join("b.txt"), b"world").expect("write sub/b.txt");

        let (temp_file, size) = tar_file_stream("**/*", root, &[], false).expect("tar_file_stream");

        assert!(size > 0, "archive size must be positive (got {size})");

        // Reopen at position 0 for reading.
        let mut read_fd = temp_file.reopen().expect("reopen temp file");
        let gz = flate2::read::GzDecoder::new(&mut read_fd);
        let mut archive = tar::Archive::new(gz);

        let mut entries: HashMap<String, Vec<u8>> = HashMap::new();
        let mut raw_count = 0usize;
        for entry_result in archive.entries().expect("iterate entries") {
            let mut entry = entry_result.expect("read entry");
            let name = entry
                .path()
                .expect("entry path")
                .to_string_lossy()
                .into_owned();
            let mut content = Vec::new();
            entry.read_to_end(&mut content).expect("read entry content");
            entries.insert(name, content);
            raw_count += 1;
        }

        // No duplicate archive entries: the raw iterated count must equal the
        // deduplicated key count. A double-recursion bug (e.g. `sub/b.txt`
        // appended twice) would make `raw_count > entries.len()`.
        assert_eq!(
            raw_count,
            entries.len(),
            "archive must not contain duplicate entries (raw {raw_count} vs unique {})",
            entries.len()
        );

        assert!(entries.contains_key("a.txt"), "a.txt must be in archive");
        assert_eq!(
            entries.get("a.txt").map(Vec::as_slice),
            Some(b"hello".as_slice()),
            "a.txt content must match"
        );

        assert!(
            entries.contains_key("sub/b.txt"),
            "sub/b.txt must be in archive; found keys: {:?}",
            entries.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            entries.get("sub/b.txt").map(Vec::as_slice),
            Some(b"world".as_slice()),
            "sub/b.txt content must match"
        );
    }

    /// Sanity check that `tar_file_stream` produces a non-zero archive when
    /// files are present. (`tar_file_stream` does not error on an empty file
    /// list — gzip/tar framing always yields a positive size; the empty-list
    /// check lives in `calculate_files_hash`, not here.)
    #[test]
    fn tar_size_is_positive_for_nonempty_tree() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("x.txt"), b"data").expect("write x.txt");

        let (_tmp, size) =
            tar_file_stream("**/*", dir.path(), &[], false).expect("tar_file_stream");
        assert!(size > 0, "expected positive archive size");
    }
}
