//! Atomic config writes with an explicit policy for a symlink at the destination.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::error::ImportError;
use super::items::ImportScope;

/// Most hops followed before giving up; matches the smallest common kernel limit.
const SYMLINK_FOLLOW_LIMIT: usize = 32;

static WRITE_NONCE: AtomicU64 = AtomicU64::new(0);

/// Write `contents` to `path` atomically.
///
/// A user config that is a symlink (dotfile managers) is written through, so the link
/// survives and its target gets the new content. A project config symlink is replaced
/// by a regular file, so an import never writes outside the project through a link.
pub(super) fn write_scoped(
    path: &Path,
    contents: &str,
    scope: ImportScope,
) -> Result<(), ImportError> {
    let dest = match scope {
        ImportScope::Global => follow_symlinks(path)?,
        ImportScope::Project => path.to_path_buf(),
    };
    write_atomically(&dest, contents.as_bytes())
}

fn follow_symlinks(path: &Path) -> Result<PathBuf, ImportError> {
    let mut current = path.to_path_buf();
    for _ in 0..SYMLINK_FOLLOW_LIMIT {
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = std::fs::read_link(&current)
                    .map_err(|source| ImportError::io("resolve", path, source))?;
                current = if target.is_absolute() {
                    target
                } else {
                    current
                        .parent()
                        .map_or_else(|| target.clone(), |parent| parent.join(&target))
                };
            }
            Ok(_) => return Ok(current),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(current),
            Err(source) => return Err(ImportError::io("resolve", path, source)),
        }
    }
    Err(ImportError::io(
        "resolve",
        path,
        std::io::Error::other("too many levels of symbolic links"),
    ))
}

/// Write to a unique temp file next to `dest`, then rename over it.
pub(super) fn write_atomically(dest: &Path, contents: &[u8]) -> Result<(), ImportError> {
    let parent = dest
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|source| ImportError::io("create directory", parent, source))?;
    let file_name = dest
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".to_string());
    let nonce = WRITE_NONCE.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".{file_name}.{}.{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)
            .map_err(|source| ImportError::io("create", &tmp, source))?;
        file.write_all(contents)
            .map_err(|source| ImportError::io("write", &tmp, source))?;
        file.sync_all()
            .map_err(|source| ImportError::io("sync", &tmp, source))?;
        drop(file);
        std::fs::rename(&tmp, dest).map_err(|source| ImportError::io("rename", dest, source))
    })();
    if result.is_err() {
        // A stale temp file would otherwise sit next to the config.
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
#[path = "write_tests.rs"]
mod tests;
