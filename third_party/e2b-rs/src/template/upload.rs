//! Presigned-URL upload flow for template build context archives.
//!
//! Ports `getFileUploadLink` and `uploadFile` from the E2B JS SDK
//! (`packages/js-sdk/src/template/buildApi.ts`).
//!
//! All items are `pub(crate)` — they are implementation details consumed by
//! the template builder and are not part of the public API.
//!
//! These functions are called by Task 3+ of Plan 5b (build orchestration).
//! Until those callers land, the linter sees the items as unused; the allow
//! below suppresses those false positives.
#![allow(dead_code)]

use std::time::Duration;

use crate::errors::{Error, Result};

// ─── Constants ────────────────────────────────────────────────────────────────

/// Default upload timeout: 1 hour.  S3 presigned-URL uploads can take a long
/// time for large archives; the 60s API default would break them.
///
/// Mirrors `FILE_UPLOAD_TIMEOUT_MS` in the JS SDK.
pub(crate) const FILE_UPLOAD_TIMEOUT_MS: u64 = 3_600_000;

// ─── FileUploadLink ───────────────────────────────────────────────────────────

/// Hand-written wrapper over the generated
/// [`crate::api::schema::TemplateBuildFileUpload`] wire type.
///
/// The generated type is not exposed outside this module — callers work with
/// this ergonomic wrapper instead.
pub(crate) struct FileUploadLink {
    /// Whether the file context archive is already present in the S3 cache.
    pub(crate) present: bool,
    /// Presigned S3 PUT URL.  `None` when `present` is `true` (no upload
    /// needed) or when the server omits the field.
    pub(crate) url: Option<String>,
}

impl FileUploadLink {
    /// Convert the generated wire type into a [`FileUploadLink`].
    pub(crate) fn from_wire(wire: crate::api::schema::TemplateBuildFileUpload) -> Self {
        Self {
            present: wire.present,
            url: wire.url,
        }
    }

    /// Returns `true` when the file context is already cached and no upload is
    /// required.
    pub(crate) fn present(&self) -> bool {
        self.present
    }

    /// Returns the presigned PUT URL, or `None` when the archive is already
    /// cached or no URL was returned by the server.
    pub(crate) fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }
}

// ─── get_file_upload_link ────────────────────────────────────────────────────

/// Query whether the file context archive identified by `files_hash` is
/// already cached for `template_id`, and obtain a presigned upload URL if not.
///
/// Issues `GET /templates/{template_id}/files/{files_hash}` via the
/// control-plane [`crate::api::client::ApiClient`] and wraps the response in
/// a [`FileUploadLink`].
///
/// # Errors
///
/// Propagates transport and API errors from [`crate::api::client::ApiClient`].
pub(crate) async fn get_file_upload_link(
    api: &crate::api::client::ApiClient,
    template_id: &str,
    files_hash: &str,
) -> Result<FileUploadLink> {
    let path = format!("/templates/{template_id}/files/{files_hash}");
    let wire = api
        .request::<crate::api::schema::TemplateBuildFileUpload>(
            reqwest::Method::GET,
            &path,
            &[],
            None,
        )
        .await?;
    Ok(FileUploadLink::from_wire(wire))
}

// ─── upload_file ─────────────────────────────────────────────────────────────

/// Upload the gzip-tar context archive to the given presigned S3 URL via HTTP
/// PUT.
///
/// ## Content-Length handling
///
/// S3 presigned-PUT URLs reject requests that use chunked
/// `Transfer-Encoding` with `501 Not Implemented` (see e2b-dev/e2b#1243).
/// This function reads the archive into a `Vec<u8>` and sends it as a sized
/// body so that `reqwest` can compute and send a concrete `Content-Length`
/// header.  The explicit `Content-Length: {size}` header is also set from the
/// `size` argument (which was obtained by stat-ing the archive in
/// [`crate::template::archive::tar_file_stream`]).
///
/// For most archives the in-memory buffer is acceptable.  A streaming approach
/// (using a `File` body backed by `ContentLength`) can be adopted in a future
/// iteration if very large archives need to be supported without buffering.
///
/// ## Timeout
///
/// `timeout_ms` is applied to the entire PUT request.  Callers should pass
/// [`FILE_UPLOAD_TIMEOUT_MS`] (3 600 000 ms = 1 hour) unless they have a
/// tighter budget.
///
/// # Errors
///
/// Returns [`crate::Error::FileUpload`] if the body cannot be read, if the
/// HTTP transport fails, or if the server responds with a non-2xx status.
pub(crate) async fn upload_file(
    http: &reqwest::Client,
    url: &str,
    archive: tempfile::NamedTempFile,
    size: u64,
    timeout_ms: u64,
) -> Result<()> {
    // Read into a Vec<u8> so reqwest can emit a concrete Content-Length instead
    // of chunked Transfer-Encoding (S3 rejects the latter with 501).
    let bytes = std::fs::read(archive.path())
        .map_err(|e| Error::FileUpload(format!("failed to read archive for upload: {e}")))?;

    // `archive` is no longer needed — drop it here to release the temp file
    // even if the upload blocks for a long time.
    drop(archive);

    // The buffered byte count is authoritative for `Content-Length` — deriving
    // it from `bytes.len()` (rather than the caller-supplied `size`) guarantees
    // the advertised length always matches the body actually sent, so a stale
    // `size` argument can never produce a mismatched header. They are expected
    // to agree (both originate from the same spooled archive).
    let content_length = u64::try_from(bytes.len()).unwrap_or(size);
    debug_assert_eq!(
        content_length, size,
        "upload_file: buffered archive length must match the declared size"
    );

    let resp = http
        .put(url)
        .header(reqwest::header::CONTENT_LENGTH, content_length)
        .body(reqwest::Body::from(bytes))
        .timeout(Duration::from_millis(timeout_ms))
        .send()
        .await
        .map_err(|e| Error::FileUpload(format!("upload request failed: {e}")))?;

    if resp.status().is_success() {
        return Ok(());
    }

    let status = resp.status();
    // Try to extract a server-side error message; fall back to the HTTP reason.
    let body_bytes = resp.bytes().await.unwrap_or_default();
    let message = serde_json::from_slice::<serde_json::Value>(&body_bytes)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_owned))
        .unwrap_or_else(|| {
            status
                .canonical_reason()
                .unwrap_or("upload failed")
                .to_owned()
        });

    Err(Error::FileUpload(format!(
        "upload failed with HTTP {}: {message}",
        status.as_u16()
    )))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    use crate::api::client::ApiClient;
    use crate::connection_config::{ConnectionConfig, ConnectionConfigOpts};
    use wiremock::matchers::{body_bytes, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn api_client_for(server: &MockServer) -> ApiClient {
        let config = ConnectionConfig::new(ConnectionConfigOpts {
            api_key: Some("e2b_0123456789abcdef".to_string()),
            api_url: Some(server.uri()),
            ..Default::default()
        });
        ApiClient::new(&config, true).expect("construct ApiClient")
    }

    // ── get_file_upload_link ───────────────────────────────────────────────

    #[tokio::test]
    async fn get_file_upload_link_present() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/templates/tpl_1/files/abc123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "present": true,
                "url": "https://s3.example.com/presigned"
            })))
            .mount(&server)
            .await;

        let api = api_client_for(&server);
        let link = get_file_upload_link(&api, "tpl_1", "abc123")
            .await
            .expect("get_file_upload_link");

        assert!(link.present(), "present must be true");
        assert_eq!(
            link.url(),
            Some("https://s3.example.com/presigned"),
            "url must match"
        );
    }

    #[tokio::test]
    async fn get_file_upload_link_absent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/templates/tpl_1/files/def456"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "present": false })),
            )
            .mount(&server)
            .await;

        let api = api_client_for(&server);
        let link = get_file_upload_link(&api, "tpl_1", "def456")
            .await
            .expect("get_file_upload_link");

        assert!(!link.present(), "present must be false");
        assert!(link.url().is_none(), "url must be None when absent");
    }

    // ── upload_file ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn upload_file_puts_with_content_length_and_body() {
        let server = MockServer::start().await;
        let body_content = b"fake-archive-bytes";
        let expected_len = body_content.len() as u64;

        Mock::given(method("PUT"))
            .and(path("/upload/presigned"))
            .and(header("content-length", expected_len.to_string().as_str()))
            .and(body_bytes(body_content.to_vec()))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        // Write a temp file with known content.
        let mut archive = tempfile::NamedTempFile::new().expect("NamedTempFile");
        archive.write_all(body_content).expect("write archive");
        archive.flush().expect("flush");

        let upload_url = format!("{}/upload/presigned", server.uri());
        let http = reqwest::Client::new();

        upload_file(
            &http,
            &upload_url,
            archive,
            expected_len,
            FILE_UPLOAD_TIMEOUT_MS,
        )
        .await
        .expect("upload_file must succeed for 200");
    }

    #[tokio::test]
    async fn upload_file_errors_on_non_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/upload/reject"))
            .respond_with(ResponseTemplate::new(501).set_body_json(serde_json::json!({
                "message": "Not Implemented"
            })))
            .mount(&server)
            .await;

        let mut archive = tempfile::NamedTempFile::new().expect("NamedTempFile");
        archive.write_all(b"data").expect("write");

        let upload_url = format!("{}/upload/reject", server.uri());
        let http = reqwest::Client::new();

        let err = upload_file(&http, &upload_url, archive, 4, FILE_UPLOAD_TIMEOUT_MS)
            .await
            .expect_err("must error on 501");

        assert!(
            matches!(err, Error::FileUpload(_)),
            "expected FileUpload error, got {err:?}"
        );
    }
}
