//! Read rotating HTTP MCP credentials for each request without exposing token contents in errors.

use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use http::HeaderValue;
use tokio::task::JoinHandle;

const MAX_TOKEN_FILE_BYTES: u64 = 16 * 1024;
const TOKEN_READ_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const META_KEY: &str = "supercharge/mcp/bearerTokenFile";

#[derive(Debug, Clone)]
pub struct BearerTokenFile {
    path: Arc<PathBuf>,
    stalled_read: Arc<parking_lot::Mutex<Option<JoinHandle<io::Result<String>>>>>,
    read_lock: Arc<tokio::sync::Mutex<()>>,
}

impl BearerTokenFile {
    pub fn from_meta(meta: Option<&agent_client_protocol::Meta>) -> Result<Option<Self>, String> {
        let Some(value) = meta.and_then(|meta| meta.get(META_KEY)) else {
            return Ok(None);
        };
        let configured = value
            .as_str()
            .ok_or_else(|| format!("{META_KEY} must be a string"))?;
        if configured.is_empty() {
            return Err("bearer_token_file must not be empty".to_owned());
        }
        let path = if let Some(rest) = configured.strip_prefix("~/") {
            let home = std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .ok_or_else(|| {
                    format!("bearer_token_file {configured} requires a home directory")
                })?;
            PathBuf::from(home).join(rest)
        } else {
            PathBuf::from(configured)
        };
        if !path.is_absolute() {
            return Err(format!(
                "bearer_token_file {configured} must be an absolute or ~/ path"
            ));
        }
        Ok(Some(Self::new(path)))
    }

    pub fn new(path: PathBuf) -> Self {
        Self {
            path: Arc::new(path),
            stalled_read: Arc::default(),
            read_lock: Arc::default(),
        }
    }

    pub(crate) async fn read(&self) -> io::Result<String> {
        let path = &self.path;
        let _guard = tokio::time::timeout(TOKEN_READ_TIMEOUT, self.read_lock.lock())
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "waiting to read bearer_token_file {} timed out",
                        path.display()
                    ),
                )
            })?;
        // Blocking filesystem reads cannot be cancelled; retain the handle to limit hung reads.
        if self
            .stalled_read
            .lock()
            .as_ref()
            .is_some_and(|read| !read.is_finished())
        {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "an earlier read of bearer_token_file {} has not returned",
                    path.display()
                ),
            ));
        }
        let mut read = tokio::task::spawn_blocking({
            let path = Arc::clone(path);
            move || read_token_file_sync(&path)
        });
        let contents = match tokio::time::timeout(TOKEN_READ_TIMEOUT, &mut read).await {
            Ok(joined) => joined.map_err(io::Error::other)?,
            Err(_) => {
                *self.stalled_read.lock() = Some(read);
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("reading bearer_token_file {} timed out", path.display()),
                ));
            }
        }
        .map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("bearer_token_file {}: {e}", path.display()),
            )
        })?;
        let token = contents.trim();
        if token.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("bearer_token_file {} is empty", path.display()),
            ));
        }
        if !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~+/=".contains(&byte))
            || HeaderValue::from_str(&format!("Bearer {token}")).is_err()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "bearer_token_file {} contains invalid header characters",
                    path.display()
                ),
            ));
        }
        Ok(token.to_owned())
    }
}

fn read_token_file_sync(path: &PathBuf) -> io::Result<String> {
    let mut contents = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_TOKEN_FILE_BYTES + 1)
        .read_to_end(&mut contents)?;
    if contents.len() as u64 > MAX_TOKEN_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("larger than {MAX_TOKEN_FILE_BYTES} bytes"),
        ));
    }
    String::from_utf8(contents).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_oversized_invalid_and_empty_files_fail_without_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        let file = BearerTokenFile::new(path.clone());
        assert_eq!(
            file.read().await.unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        for (contents, reason) in [
            (vec![b'x'; MAX_TOKEN_FILE_BYTES as usize + 1], "larger"),
            (vec![0xff], "UTF-8"),
            (b"  \n".to_vec(), "empty"),
            (b"secret\rinside".to_vec(), "invalid header"),
            (b"secret inside".to_vec(), "invalid header"),
        ] {
            std::fs::write(&path, &contents).unwrap();
            let error = file.read().await.unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(error.to_string().contains(reason));
            assert!(!error.to_string().contains("secret"));
        }
    }

    #[test]
    fn rejects_relative_and_non_string_meta() {
        for value in [
            serde_json::json!("token"),
            serde_json::json!(""),
            serde_json::json!(4),
        ] {
            let meta = agent_client_protocol::Meta::from_iter([(META_KEY.to_owned(), value)]);
            assert!(BearerTokenFile::from_meta(Some(&meta)).is_err());
        }
    }
}
