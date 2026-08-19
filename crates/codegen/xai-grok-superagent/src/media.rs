use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaAttachment {
    pub path: PathBuf,
    pub file_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
}

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("media path is invalid")]
    InvalidPath,
    #[error("media path is outside the configured root")]
    OutsideRoot,
    #[error("media file is too large")]
    TooLarge,
    #[error("media file metadata failed: {0}")]
    Io(#[from] std::io::Error),
}

#[allow(clippy::disallowed_methods)]
pub fn extract_media(
    response: &str,
    allowed_root: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<(String, Vec<MediaAttachment>), MediaError> {
    let root = allowed_root.as_ref();
    let mut clean_lines = Vec::new();
    let mut media = Vec::new();
    for line in response.lines() {
        let Some(spec) = line.strip_prefix("MEDIA:") else {
            clean_lines.push(line);
            continue;
        };
        let spec = spec.trim();
        if spec.is_empty() {
            return Err(MediaError::InvalidPath);
        }
        let (raw_path, mime_type) = spec
            .split_once('|')
            .unwrap_or((spec, "application/octet-stream"));
        let raw_path = raw_path.trim();
        let path = Path::new(raw_path);
        if raw_path.is_empty()
            || path.is_absolute()
            || path
                .components()
                .any(|component| component == std::path::Component::ParentDir)
        {
            return Err(MediaError::InvalidPath);
        }
        let resolved = root.join(path);
        let root_canonical = canonicalize_allow_missing(root)?;
        let resolved_canonical = fs::canonicalize(&resolved).map_err(MediaError::Io)?;
        if !resolved_canonical.starts_with(&root_canonical) {
            return Err(MediaError::OutsideRoot);
        }
        let metadata = fs::metadata(&resolved_canonical)?;
        if !metadata.is_file() || metadata.len() > max_bytes {
            return Err(MediaError::TooLarge);
        }
        let file_name = resolved_canonical
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(MediaError::InvalidPath)?
            .to_owned();
        media.push(MediaAttachment {
            path: resolved_canonical,
            file_name,
            mime_type: mime_type.trim().to_owned(),
            size_bytes: metadata.len(),
        });
    }
    Ok((clean_lines.join("\n").trim().to_owned(), media))
}

#[allow(clippy::disallowed_methods)]
fn canonicalize_allow_missing(path: &Path) -> Result<PathBuf, MediaError> {
    fs::canonicalize(path).map_err(MediaError::Io)
}

#[cfg(test)]
mod tests {
    use super::{MediaError, extract_media};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn extracts_and_validates_media_markers() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("report.txt"), "hello").unwrap();
        let (text, media) =
            extract_media("Answer\nMEDIA:report.txt|text/plain", dir.path(), 100).unwrap();
        assert_eq!(text, "Answer");
        assert_eq!(media[0].file_name, "report.txt");
        assert_eq!(media[0].mime_type, "text/plain");
        assert!(matches!(
            extract_media("MEDIA:../secret", dir.path(), 100),
            Err(MediaError::InvalidPath)
        ));
    }
}
