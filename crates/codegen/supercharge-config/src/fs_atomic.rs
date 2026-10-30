//! Atomic file writes, shared by the managed-cache marker, the signature sidecar, and downstream identifier caches (e.g. the telemetry agent id).
//! The temp file is fsynced before it is published, so a crash cannot leave the final path holding a truncated file.

use std::path::Path;

/// Write to a temp file then rename, so a torn write can't leave a half-written file.
/// The temp name is unique per writer (pid and counter) and `create_new`, so concurrent writers don't collide.
/// `mode` (unix only) is applied at temp-file creation, so the final file never exists with looser permissions.
pub fn write_atomically(
    final_path: &Path,
    contents: &str,
    mode: Option<u32>,
) -> std::io::Result<()> {
    write_via_temp(final_path, contents, mode, |tmp, final_path| {
        std::fs::rename(tmp, final_path)
    })
}

/// [`write_atomically`], but the write lands only when `final_path` does not exist yet.
/// `hard_link` refuses an existing target where `rename` would replace it, so of several concurrent
/// first writers exactly one wins; the rest get `AlreadyExists` and the winner's file is untouched.
/// On a filesystem without hard links the write still lands, without that guarantee.
pub fn write_atomically_if_absent(
    final_path: &Path,
    contents: &str,
    mode: Option<u32>,
) -> std::io::Result<()> {
    write_via_temp(final_path, contents, mode, |tmp, final_path| {
        let linked = match std::fs::hard_link(tmp, final_path) {
            // No hard links here (exFAT/FAT32, some network mounts): rename lands the file, but a concurrent first writer can replace it
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::Unsupported | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                return std::fs::rename(tmp, final_path);
            }
            linked => linked,
        };
        let _ = std::fs::remove_file(tmp);
        linked
    })
}

/// [`write_atomically`], and the file renamed into place must be the temp this call wrote, as its
/// open handle identifies it: a file swapped in under the temp's name is removed rather than
/// published, and the call fails with `InvalidData`. Where std exposes no inode, the rename is trusted.
pub fn write_atomically_verified(
    final_path: &Path,
    contents: &str,
    mode: Option<u32>,
) -> std::io::Result<()> {
    write_via_temp_with(
        final_path,
        contents,
        mode,
        std::fs::File::metadata,
        |tmp, path, created| {
            std::fs::rename(tmp, path)?;
            if std::fs::symlink_metadata(path).is_ok_and(|landed| same_file(&landed, &created)) {
                return Ok(());
            }
            let _ = std::fs::remove_file(path);
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} was swapped for another file while it was written", path.display()),
            ))
        },
    )
}

fn same_file(landed: &std::fs::Metadata, created: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        landed.is_file() && landed.dev() == created.dev() && landed.ino() == created.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = created;
        landed.is_file()
    }
}

fn write_via_temp(
    final_path: &Path,
    contents: &str,
    mode: Option<u32>,
    publish: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    write_via_temp_with(final_path, contents, mode, |_| Ok(()), |tmp, path, ()| publish(tmp, path))
}

fn write_via_temp_with<T>(
    final_path: &Path,
    contents: &str,
    mode: Option<u32>,
    inspect: impl FnOnce(&std::fs::File) -> std::io::Result<T>,
    publish: impl FnOnce(&Path, &Path, T) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    static WRITE_NONCE: AtomicU64 = AtomicU64::new(0);

    let dir = final_path.parent().unwrap_or_else(|| Path::new("."));
    let name = final_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_owned());
    let nonce = WRITE_NONCE.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!("{name}.{}.{nonce}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let result = options
        .open(&tmp)
        .and_then(|mut f| {
            f.write_all(contents.as_bytes())?;
            f.sync_all()?;
            inspect(&f)
        })
        .and_then(|seen| publish(&tmp, final_path, seen));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
#[path = "fs_atomic_tests.rs"]
mod tests;
