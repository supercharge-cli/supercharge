use std::{env, path::Path, time::Duration};

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::media::{MediaAttachment, extract_media};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShareLink {
    pub name: String,
    pub url: String,
    pub mime_type: String,
    pub size_bytes: u64,
}

#[derive(Debug, Error)]
pub enum ShareError {
    #[error("media response contains an invalid workspace path")]
    InvalidPath,
    #[error("file-share service is not configured")]
    ServiceNotConfigured,
    #[error("file-share service request failed: {0}")]
    Request(String),
    #[error("file-share service returned an invalid response")]
    InvalidResponse,
    #[error("media parser failed: {0}")]
    Media(#[from] crate::media::MediaError),
}

#[derive(Debug, Deserialize)]
struct ShareResponse {
    #[serde(default)]
    ok: bool,
    token: Option<String>,
    name: Option<String>,
    url: Option<String>,
    error: Option<String>,
}

fn share_endpoint() -> Option<String> {
    env::var("SUPERAGENT_SHARE_SERVICE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            let port = env::var("SUPERAGENT_WAKE_PORT").unwrap_or_else(|_| "8788".to_owned());
            Some(format!("http://127.0.0.1:{port}/share"))
        })
}

fn share_secret() -> Option<String> {
    [
        "SUPERAGENT_APP_INBOX_SECRET",
        "SUPERAGENT_GATEWAY_TOKEN",
        "XIBECODE_APP_INBOX_SECRET",
        "XIBECODE_GATEWAY_TOKEN",
    ]
    .into_iter()
    .find_map(|key| env::var(key).ok().filter(|value| !value.trim().is_empty()))
}

#[allow(clippy::disallowed_methods)]
fn workspace_relative(path: &Path, root: &Path) -> Result<String, ShareError> {
    let root = std::fs::canonicalize(root).map_err(|_| ShareError::InvalidPath)?;
    let path = std::fs::canonicalize(path).map_err(|_| ShareError::InvalidPath)?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err(ShareError::InvalidPath);
    }
    path.strip_prefix(root)
        .ok()
        .and_then(|value| value.to_str())
        .map(|value| value.replace('\\', "/"))
        .filter(|value| !value.is_empty() && !value.contains(".."))
        .ok_or(ShareError::InvalidPath)
}

pub async fn create_share(
    attachment: &MediaAttachment,
    workspace_root: impl AsRef<Path>,
) -> Result<ShareLink, ShareError> {
    let relative = workspace_relative(&attachment.path, workspace_root.as_ref())?;
    let endpoint = share_endpoint().ok_or(ShareError::ServiceNotConfigured)?;
    let client = crate::http_client(|builder| builder.timeout(Duration::from_secs(20)))
        .map_err(|error| ShareError::Request(error.to_string()))?;
    let mut request = client.post(endpoint).json(&serde_json::json!({
        "path": relative,
        "name": attachment.file_name,
    }));
    if let Some(secret) = share_secret() {
        request = request.bearer_auth(secret);
    }
    let response = request
        .send()
        .await
        .map_err(|error| ShareError::Request(error.to_string()))?;
    let status = response.status();
    let body = response
        .json::<ShareResponse>()
        .await
        .map_err(|_| ShareError::InvalidResponse)?;
    if status != StatusCode::OK || !body.ok {
        return Err(ShareError::Request(
            body.error.unwrap_or_else(|| format!("HTTP {status}")),
        ));
    }
    let url = body.url.ok_or(ShareError::InvalidResponse)?;
    let _token = body.token.ok_or(ShareError::InvalidResponse)?;
    Ok(ShareLink {
        name: body.name.unwrap_or_else(|| attachment.file_name.clone()),
        url,
        mime_type: attachment.mime_type.clone(),
        size_bytes: attachment.size_bytes,
    })
}

pub async fn rewrite_media_links(
    response: &str,
    workspace_root: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<String, ShareError> {
    if !response
        .lines()
        .any(|line| line.trim_start().starts_with("MEDIA:"))
    {
        return Ok(response.to_owned());
    }
    let (text, attachments) = extract_media(response, &workspace_root, max_bytes)?;
    let mut links = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let link = create_share(&attachment, workspace_root.as_ref()).await?;
        links.push(format!("- {}: {}", link.name, link.url));
    }
    if links.is_empty() {
        Ok(text)
    } else if text.is_empty() {
        Ok(format!("Files:\n{}", links.join("\n")))
    } else {
        Ok(format!("{text}\n\nFiles:\n{}", links.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::workspace_relative;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn workspace_relative_rejects_escape() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("ok.txt"), "ok").unwrap();
        assert_eq!(
            workspace_relative(&dir.path().join("ok.txt"), dir.path()).unwrap(),
            "ok.txt"
        );
        assert!(workspace_relative(&dir.path().join("missing"), dir.path()).is_err());
    }
}
