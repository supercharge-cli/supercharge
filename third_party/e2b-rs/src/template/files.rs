//! File discovery and content-hashing for template build context.
//!
//! Ports `validateRelativePath`, `readDockerignore`, `getAllFilesInPath`, and
//! `calculateFilesHash` from the E2B JS SDK
//! (`packages/js-sdk/src/template/utils.ts`).
//!
//! All items are `pub(crate)` — they are implementation details consumed by
//! the template builder and are not part of the public API.
//!
//! The functions in this module are called by Tasks 2–4 of Plan 5b (tar
//! streaming, build upload, etc.).  Until those callers land, the linter sees
//! the items as unused; the allow below suppresses those false positives.
#![allow(dead_code)]

use std::path::{Component, Path, PathBuf};

use globset::{Glob, GlobSetBuilder};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::errors::{Error, Result};

// ─── validate_relative_path ──────────────────────────────────────────────────

/// Validate that `src` is a relative path that stays within the context
/// directory.
///
/// Mirrors `validateRelativePath` in the JS SDK. Rejects:
/// - Absolute paths (e.g. `/absolute`, `C:\Windows`).
/// - Paths that escape the context after logical normalization
///   (e.g. `../foo`, `foo/../../bar`, `./foo/../../../bar`).
///
/// Accepts paths like `foo/bar`, `./foo`, `foo/../bar` (does not escape).
///
/// # Errors
///
/// Returns [`crate::Error::InvalidArgument`] for invalid paths, with error
/// messages that match the JS SDK exactly.
pub(crate) fn validate_relative_path(src: &str) -> Result<()> {
    if Path::new(src).is_absolute() {
        return Err(Error::InvalidArgument(format!(
            "Invalid source path \"{src}\": absolute paths are not allowed. \
             Use a relative path within the context directory."
        )));
    }

    if path_escapes_context(src) {
        return Err(Error::InvalidArgument(format!(
            "Invalid source path \"{src}\": path escapes the context directory. \
             The path must stay within the context directory."
        )));
    }

    Ok(())
}

/// Return `true` if the path (logically normalised) would escape the context
/// directory via `..` segments.
///
/// Tracks a running depth counter: `..` decrements it; `Normal` increments it.
/// The path escapes if the depth ever goes negative.
fn path_escapes_context(src: &str) -> bool {
    let mut depth: i64 = 0;
    for component in Path::new(src).components() {
        match component {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            Component::Normal(_) => {
                depth += 1;
            }
            // CurDir (`.`), RootDir, Prefix — do not change depth.
            _ => {}
        }
    }
    false
}

// ─── read_dockerignore ────────────────────────────────────────────────────────

/// Read and parse `{context}/.dockerignore`.
///
/// Returns an empty [`Vec`] if the file does not exist or cannot be read.
/// Filters out blank lines and lines starting with `#` (comments).
/// Trims leading/trailing whitespace from each line.
///
/// Mirrors `readDockerignore` in the JS SDK.
pub(crate) fn read_dockerignore(context: &Path) -> Vec<String> {
    let dockerignore_path = context.join(".dockerignore");
    match std::fs::read_to_string(&dockerignore_path) {
        Ok(content) => content
            .lines()
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect(),
        Err(_) => Vec::new(),
    }
}

// ─── get_all_files_in_path ────────────────────────────────────────────────────

/// Expand `src` as a glob pattern within `context`, apply `ignore` patterns,
/// and return a **sorted, deduplicated** list of matching paths.
///
/// - If a matched entry is a **directory**, all entries inside it are also
///   included recursively (mirrors the JS `dir/**/*` expansion).
/// - `ignore` patterns are matched against the **context-relative** path using
///   [`globset`]. A path is excluded if it matches *any* pattern.
/// - The returned [`Vec`] is sorted lexicographically (deterministic order is
///   essential for [`calculate_files_hash`]).
///
/// Mirrors `getAllFilesInPath` in the JS SDK.
///
/// # Errors
///
/// Returns [`crate::Error::InvalidArgument`] for malformed glob or ignore
/// patterns, and [`crate::Error::Internal`] for I/O errors during traversal.
pub(crate) fn get_all_files_in_path(
    src: &str,
    context: &Path,
    ignore: &[String],
) -> Result<Vec<PathBuf>> {
    let ignore_set = build_ignore_set(ignore)?;
    let is_ignored = |rel: &Path| ignore_set.is_match(rel);

    // BTreeSet gives free dedup + sort.
    let mut files: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();

    let full_pattern = context.join(src).to_string_lossy().into_owned();
    let opts = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: false,
        // dot: true equivalent — allow * to match hidden files.
        require_literal_leading_dot: false,
    };

    let glob_iter = glob::glob_with(&full_pattern, opts)
        .map_err(|e| Error::InvalidArgument(format!("invalid glob pattern \"{src}\": {e}")))?;

    for entry in glob_iter {
        let path = entry.map_err(|e| Error::Internal(format!("glob traversal error: {e}")))?;

        let rel = path
            .strip_prefix(context)
            .map_err(|e| Error::Internal(format!("unexpected prefix error: {e}")))?;

        if is_ignored(rel) {
            continue;
        }

        // Use symlink_metadata (lstat) so a symlink-to-dir is NOT treated as a dir.
        let metadata = path
            .symlink_metadata()
            .map_err(|e| Error::Internal(format!("failed to stat \"{}\": {e}", path.display())))?;

        if metadata.is_dir() {
            // Include the directory entry itself.
            files.insert(path.clone());

            // Walk and include all nested entries. Propagate traversal
            // errors (e.g. permission-denied on a subdirectory) rather than
            // swallowing them — a silently truncated file list would produce
            // a wrong (incomplete) cache hash, not an error.
            for walk_result in WalkDir::new(&path).follow_links(false).into_iter().skip(1)
            // the root dir is already inserted above
            {
                let walk_entry = walk_result
                    .map_err(|e| Error::Internal(format!("directory traversal error: {e}")))?;
                let entry_path = walk_entry.path().to_path_buf();
                let entry_rel = entry_path
                    .strip_prefix(context)
                    .map_err(|e| Error::Internal(format!("unexpected prefix error: {e}")))?;
                if !is_ignored(entry_rel) {
                    files.insert(entry_path);
                }
            }
        } else {
            files.insert(path);
        }
    }

    Ok(files.into_iter().collect())
}

/// Build a [`globset::GlobSet`] from a slice of pattern strings.
fn build_ignore_set(patterns: &[String]) -> Result<globset::GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let g = Glob::new(pattern).map_err(|e| {
            Error::InvalidArgument(format!("invalid ignore pattern \"{pattern}\": {e}"))
        })?;
        builder.add(g);
    }
    builder
        .build()
        .map_err(|e| Error::Internal(format!("failed to build GlobSet: {e}")))
}

// ─── relative_posix ──────────────────────────────────────────────────────────

/// Return the context-relative POSIX path of `path` (forward slashes).
///
/// Strips the `context` prefix from `path` and replaces any back-slashes with
/// forward slashes so the result is a valid POSIX tar entry name on all
/// platforms.
///
/// Used by [`calculate_files_hash`] and [`crate::template::archive`].
///
/// # Errors
///
/// Returns [`crate::Error::Internal`] if `path` is not under `context`.
pub(crate) fn relative_posix(path: &Path, context: &Path) -> Result<String> {
    let rel = path.strip_prefix(context).map_err(|e| {
        Error::Internal(format!(
            "path \"{}\" is not under context \"{}\": {e}",
            path.display(),
            context.display()
        ))
    })?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

// ─── calculate_files_hash ─────────────────────────────────────────────────────

/// Compute a deterministic SHA-256 (hex) hash of the files matching `src`
/// within `context`.
///
/// ## Byte sequence (algorithm pin)
///
/// 1. `"COPY {src} {dest}"` (UTF-8 bytes).
/// 2. For each path returned by [`get_all_files_in_path`] (sorted order):
///    a. The context-relative POSIX path (forward slashes, UTF-8).
///    b. The raw unix `st_mode` as a decimal string (see note below).
///    c. The byte size as a decimal string (`st_size` / [`Metadata::len`]).
///    d. **Symlink not followed**: the [`std::fs::read_link`] target string.
///    e. **Regular file**: the raw byte content ([`std::fs::read`]).
///
/// `uid`, `gid`, and `mtime` are **never** hashed — this keeps the digest
/// stable across build environments.
///
/// ## Symlink handling
///
/// A symlink is "followed" only when `resolve_symlinks` is `true` **and** its
/// target exists and is a file or directory.  Otherwise the symlink itself is
/// hashed (lstat mode/size + link target string).
///
/// ## Non-Unix mode fallback
///
/// On non-Unix targets [`std::os::unix::fs::MetadataExt`] is unavailable.
/// A stable constant of `0` is substituted for the mode value so the hash
/// remains deterministic.
/// <!-- NOTE: non-unix mode fallback -->
///
/// Mirrors `calculateFilesHash` in the JS SDK.
///
/// # Errors
///
/// - [`crate::Error::InvalidArgument`] for malformed patterns.
/// - [`crate::Error::Internal`] if no files match `src`, or on I/O errors.
pub(crate) fn calculate_files_hash(
    src: &str,
    dest: &str,
    context: &Path,
    ignore: &[String],
    resolve_symlinks: bool,
) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(format!("COPY {src} {dest}"));

    let files = get_all_files_in_path(src, context, ignore)?;
    if files.is_empty() {
        return Err(Error::Internal(format!(
            "No files found in {}",
            context.join(src).display()
        )));
    }

    for path in &files {
        // Context-relative POSIX path with forward slashes.
        let rel_posix = relative_posix(path, context)?;
        hasher.update(rel_posix.as_bytes());

        // Detect symlinks via lstat.
        let lstat = path
            .symlink_metadata()
            .map_err(|e| Error::Internal(format!("failed to lstat \"{}\": {e}", path.display())))?;

        if lstat.file_type().is_symlink() {
            // Try to stat the target (may be `None` for broken symlinks).
            let stat_opt = std::fs::metadata(path).ok();
            let should_follow =
                resolve_symlinks && stat_opt.as_ref().is_some_and(|s| s.is_file() || s.is_dir());

            if !should_follow {
                // Hash lstat mode/size + link target string.
                hash_mode_size(&mut hasher, &lstat);
                let target = std::fs::read_link(path).map_err(|e| {
                    Error::Internal(format!("readlink \"{}\": {e}", path.display()))
                })?;
                hasher.update(target.to_string_lossy().as_bytes());
                continue;
            }
        }

        // Regular stat (follows symlinks if present).
        let stat = std::fs::metadata(path)
            .map_err(|e| Error::Internal(format!("failed to stat \"{}\": {e}", path.display())))?;
        hash_mode_size(&mut hasher, &stat);

        if stat.is_file() {
            let content = std::fs::read(path).map_err(|e| {
                Error::Internal(format!("failed to read \"{}\": {e}", path.display()))
            })?;
            hasher.update(&content);
        }
    }

    Ok(format!("{:x}", hasher.finalize()))
}

/// Feed `mode.to_string()` then `size.to_string()` into `hasher`.
///
/// On Unix the raw `st_mode` is used (includes file-type bits, e.g. `33188`
/// for a regular file with permissions `0o644`).
/// On non-Unix a stable fallback of `0` is used.
/// <!-- NOTE: non-unix mode fallback -->
fn hash_mode_size(hasher: &mut Sha256, metadata: &std::fs::Metadata) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hasher.update(metadata.mode().to_string());
    }
    #[cfg(not(unix))]
    {
        // NOTE: non-unix mode fallback — st_mode is not available on this
        // platform; substitute 0 so the hash is still deterministic.
        let _ = metadata;
        hasher.update("0");
    }
    hasher.update(metadata.len().to_string());
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::fs;

    // ── validate_relative_path ─────────────────────────────────────────────

    #[test]
    fn validate_relative_path_rejects_absolute_and_escape() {
        // Absolute paths.
        assert!(
            matches!(
                validate_relative_path("/absolute/path"),
                Err(Error::InvalidArgument(_))
            ),
            "absolute path must be rejected"
        );

        // Path that escapes via `..`.
        assert!(
            matches!(
                validate_relative_path("../escape"),
                Err(Error::InvalidArgument(_))
            ),
            "../escape must be rejected"
        );

        // Escape via nested `..`.
        assert!(
            matches!(
                validate_relative_path("foo/../../bar"),
                Err(Error::InvalidArgument(_))
            ),
            "foo/../../bar must be rejected"
        );

        // Escape via ./..
        assert!(
            matches!(
                validate_relative_path("./foo/../../../bar"),
                Err(Error::InvalidArgument(_))
            ),
            "./foo/../../../bar must be rejected"
        );

        // Valid paths.
        assert!(
            validate_relative_path("foo/bar").is_ok(),
            "foo/bar is valid"
        );
        assert!(validate_relative_path("./foo").is_ok(), "./foo is valid");
        assert!(
            validate_relative_path("foo/../bar").is_ok(),
            "foo/../bar stays within context and is valid"
        );
        // A filename that starts with `..` but is not a directory traversal.
        assert!(
            validate_relative_path("..myconfig").is_ok(),
            "..myconfig is a valid filename"
        );
    }

    // ── read_dockerignore ──────────────────────────────────────────────────

    #[test]
    fn read_dockerignore_filters_comments_and_blanks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ignore_path = dir.path().join(".dockerignore");

        fs::write(
            &ignore_path,
            "# This is a comment\n\nnode_modules\n*.log\n  # indented comment\n  target  \n",
        )
        .expect("write .dockerignore");

        let patterns = read_dockerignore(dir.path());
        assert_eq!(patterns, vec!["node_modules", "*.log", "target"]);
    }

    #[test]
    fn read_dockerignore_missing_file_returns_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let patterns = read_dockerignore(dir.path());
        assert!(patterns.is_empty());
    }

    // ── get_all_files_in_path ──────────────────────────────────────────────

    #[test]
    fn get_all_files_sorted_and_deduped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        // Build tree: a.txt, sub/b.txt
        fs::write(root.join("a.txt"), "hello").expect("a.txt");
        fs::create_dir(root.join("sub")).expect("sub");
        fs::write(root.join("sub").join("b.txt"), "world").expect("sub/b.txt");

        // Without ignore — expect both files, sorted.
        let files = get_all_files_in_path("**/*", root, &[]).expect("get_all_files");
        let rels: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .expect("strip")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        // Should include the `sub` directory itself + both files, sorted.
        assert!(rels.contains(&"a.txt".to_owned()), "a.txt must be present");
        assert!(
            rels.contains(&"sub/b.txt".to_owned()),
            "sub/b.txt must be present"
        );
        // Result must be sorted.
        let mut sorted = rels.clone();
        sorted.sort();
        assert_eq!(rels, sorted, "paths must be in sorted order");

        // With ignore pattern `sub/**` — sub/b.txt should be excluded.
        // Note: the `sub` directory entry itself is NOT matched by `sub/**`
        // (which requires at least one sub-path component), so it may still
        // appear in the list — matching the JS `glob` / minimatch behavior.
        let ignore = vec!["sub/**".to_owned()];
        let files2 =
            get_all_files_in_path("**/*", root, &ignore).expect("get_all_files with ignore");
        let rels2: Vec<String> = files2
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .expect("strip")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        assert!(
            rels2.contains(&"a.txt".to_owned()),
            "a.txt must still be present"
        );
        assert!(
            !rels2.contains(&"sub/b.txt".to_owned()),
            "sub/b.txt must be excluded by the sub/** ignore pattern"
        );
    }

    // ── files_hash_is_deterministic ────────────────────────────────────────

    #[test]
    fn files_hash_is_deterministic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        fs::write(root.join("file.txt"), "deterministic content").expect("write file");

        // Use **/* which matches files recursively (** alone matches dirs only
        // in the Rust glob crate).
        let hash1 = calculate_files_hash("**/*", "/dst", root, &[], false).expect("hash1");
        let hash2 = calculate_files_hash("**/*", "/dst", root, &[], false).expect("hash2");

        assert_eq!(hash1, hash2, "same tree must produce identical hashes");

        // Changing content must change the hash.
        fs::write(root.join("file.txt"), "CHANGED content").expect("overwrite file");

        let hash3 = calculate_files_hash("**/*", "/dst", root, &[], false).expect("hash3");

        assert_ne!(hash1, hash3, "content change must produce a different hash");
    }

    // ── files_hash_byte_sequence (algorithm-pinning test) ─────────────────

    /// Build a one-file tree with known content, compute `calculate_files_hash`,
    /// then INDEPENDENTLY re-derive the digest from the exact byte sequence
    /// specified in the algorithm (to pin the implementation to the spec).
    #[test]
    fn files_hash_byte_sequence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        let filename = "hello.txt";
        let file_content = b"hello world";
        let src = filename;
        let dest = "/app/hello.txt";

        fs::write(root.join(filename), file_content).expect("write file");

        // Call the implementation under test.
        let actual_hash =
            calculate_files_hash(src, dest, root, &[], false).expect("calculate hash");

        // Independently derive the expected hash from the spec byte sequence.
        let file_path = root.join(filename);
        let metadata = fs::metadata(&file_path).expect("metadata");

        // Relative POSIX path (just the filename since it's at the root).
        let rel_posix = filename;

        // Raw unix st_mode as decimal string.
        #[cfg(unix)]
        let mode_str = {
            use std::os::unix::fs::MetadataExt;
            metadata.mode().to_string()
        };
        #[cfg(not(unix))]
        let mode_str = "0".to_owned();

        let size_str = metadata.len().to_string();

        let mut expected_hasher = Sha256::new();
        // 1. "COPY {src} {dest}"
        expected_hasher.update(format!("COPY {src} {dest}"));
        // 2. relative posix path
        expected_hasher.update(rel_posix.as_bytes());
        // 3. mode
        expected_hasher.update(mode_str.as_bytes());
        // 4. size
        expected_hasher.update(size_str.as_bytes());
        // 5. file content
        expected_hasher.update(file_content);

        let expected_hash = format!("{:x}", expected_hasher.finalize());

        assert_eq!(
            actual_hash, expected_hash,
            "hash byte sequence must match the algorithm spec exactly"
        );
    }
}
