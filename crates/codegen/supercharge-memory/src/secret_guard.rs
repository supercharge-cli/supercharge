//! Credential guard and owner-only permissions for every memory write path.
//!
//! Memory is plain local files that persist across sessions, so it must never
//! hold API keys, tokens, or passwords, and nobody but the owner may read it.

use std::path::Path;

/// Kind of the first secret-looking value in `text`, if any.
pub fn find_secret(text: &str) -> Option<&'static str> {
    supercharge_secrets::find_secret(text)
}

/// [`find_secret`] for raw file contents; non-UTF-8 bytes are checked lossily.
pub fn find_secret_bytes(contents: &[u8]) -> Option<&'static str> {
    find_secret(&String::from_utf8_lossy(contents))
}

/// Restrict a memory directory to its owner (0700 on unix; no-op elsewhere).
pub fn restrict_dir(path: &Path) {
    set_mode(path, 0o700);
}

/// Restrict a memory file to its owner (0600 on unix; no-op elsewhere).
pub fn restrict_file(path: &Path) {
    set_mode(path, 0o600);
}

/// Restrict `target` and every directory between `root` and it, `root` included.
pub fn restrict_dir_chain(root: &Path, target: &Path) {
    restrict_dir(root);
    let Ok(relative) = target.strip_prefix(root) else {
        return;
    };
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        restrict_dir(&current);
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt as _;
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() || metadata.permissions().mode() & 0o777 == mode {
        return;
    }
    if let Err(error) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
        tracing::debug!(path = %path.display(), %error, "could not restrict memory permissions");
    }
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}
