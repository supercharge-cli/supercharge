//! Byte I/O for the sandbox filesystem (`read`/`write` over envd `/files`).

use std::collections::BTreeMap;

use futures::StreamExt as _;

use super::{Filesystem, WriteEntry, WriteInfo, file_not_found_on_missing, types::FileType};
use crate::envd::rest::WriteInfoWire;
use crate::envd::versions::{ENVD_FILE_METADATA, ENVD_OCTET_STREAM_UPLOAD, version_gte};
use crate::errors::{Error, Result};

/// Header prefix for user metadata, mirroring the JS SDK.
const METADATA_HEADER_PREFIX: &str = "X-Metadata-";

/// Options for [`Filesystem::write`] / [`Filesystem::write_files`].
#[derive(Default)]
pub struct FsWriteOpts {
    /// The sandbox user to write as.
    pub user: Option<String>,
    /// Metadata to persist on the uploaded file(s) (requires envd >= 0.6.2).
    pub metadata: BTreeMap<String, String>,
    /// Gzip-compress the upload (implies octet-stream; requires envd >= 0.5.7).
    pub gzip: bool,
    /// Force octet-stream (`Some(true)`) or multipart (`Some(false)`); `None` =
    /// auto (octet-stream when `gzip`, else multipart).
    pub use_octet_stream: Option<bool>,
}

/// Validate metadata keys (RFC 7230 token chars) and values (printable US-ASCII,
/// `\x20..=\x7e`), matching the JS SDK's `validateMetadata`.
fn validate_metadata(metadata: &BTreeMap<String, String>) -> Result<()> {
    fn is_token_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
    }
    for (k, v) in metadata {
        if k.is_empty() || !k.chars().all(is_token_char) {
            return Err(Error::InvalidArgument(format!(
                "Invalid metadata key {k:?}"
            )));
        }
        if !v.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
            return Err(Error::InvalidArgument(format!(
                "Invalid metadata value for key {k:?} (must be printable US-ASCII)"
            )));
        }
    }
    Ok(())
}

/// Map the `POST /files` REST response entry to the public [`WriteInfo`]. The
/// REST `type` enum is lowercase `"file"`/`"dir"` (the envd REST schema), NOT the
/// Connect proto's `FILE_TYPE_*` names.
fn map_write_info(w: WriteInfoWire) -> WriteInfo {
    WriteInfo {
        name: w.name,
        path: w.path,
        r#type: w.type_.and_then(|t| match t.as_str() {
            "file" => Some(FileType::File),
            "dir" | "directory" => Some(FileType::Dir),
            _ => None,
        }),
        metadata: w.metadata.unwrap_or_default().into_iter().collect(),
    }
}

impl Filesystem {
    /// Read a file as UTF-8 text. Gzip-encoded responses are transparently
    /// decompressed (reqwest's `gzip` feature), so no caller-side toggle is needed.
    pub async fn read(&self, path: &str, user: Option<&str>) -> Result<String> {
        let bytes = self.read_bytes(path, user).await?;
        String::from_utf8(bytes)
            .map_err(|e| Error::Internal(format!("file {path} is not valid UTF-8: {e}")))
    }

    /// Read a file as raw bytes.
    pub async fn read_bytes(&self, path: &str, user: Option<&str>) -> Result<Vec<u8>> {
        let user = self.resolve_user(user);
        let resp = self
            .rest
            .get_files(path, user.as_deref(), false)
            .await
            .map_err(|e| file_not_found_on_missing(e, path))?;
        let bytes = resp.bytes().await?;
        Ok(bytes.to_vec())
    }

    /// Read a file as a stream of byte chunks (for large files). The Global
    /// Constraints allow byte-body reads to be `impl Stream` (no background
    /// task / channel needed — the response body IS already a stream).
    pub async fn read_stream(
        &self,
        path: &str,
        user: Option<&str>,
    ) -> Result<impl futures::Stream<Item = Result<bytes::Bytes>>> {
        let user = self.resolve_user(user);
        let resp = self
            .rest
            .get_files(path, user.as_deref(), false)
            .await
            .map_err(|e| file_not_found_on_missing(e, path))?;
        Ok(resp.bytes_stream().map(|chunk| chunk.map_err(Error::from)))
    }

    /// Common write path: validate metadata + build the `X-Metadata-*` headers.
    /// Only the metadata feature is gated as an error (envd >= 0.6.2, matching the
    /// JS SDK's `TemplateError`); octet-stream/gzip support is NOT gated here —
    /// callers silently fall back to multipart on older envd (see [`Filesystem::write`]).
    fn write_headers(&self, opts: &FsWriteOpts) -> Result<Vec<(String, String)>> {
        if !opts.metadata.is_empty() && !version_gte(&self.envd_version, ENVD_FILE_METADATA) {
            return Err(Error::Template(format!(
                "File metadata requires a newer template (envd >= {ENVD_FILE_METADATA})"
            )));
        }
        validate_metadata(&opts.metadata)?;
        Ok(opts
            .metadata
            .iter()
            .map(|(k, v)| (format!("{METADATA_HEADER_PREFIX}{k}"), v.clone()))
            .collect())
    }

    /// Write a single file, returning its [`WriteInfo`].
    ///
    /// Uses `application/octet-stream` when `use_octet_stream` is set or `gzip`
    /// is requested AND the sandbox's envd supports it (>= 0.5.7); otherwise it
    /// silently falls back to `multipart/form-data` (gzip is dropped), matching
    /// the JS SDK — it does NOT error on an old template.
    pub async fn write(
        &self,
        path: &str,
        data: impl Into<Vec<u8>>,
        opts: FsWriteOpts,
    ) -> Result<WriteInfo> {
        let headers = self.write_headers(&opts)?;
        let user = self.resolve_user(opts.user.as_deref());
        let data = data.into();
        // octet-stream when forced or gzip-requested, but only if supported by
        // this envd; otherwise fall back to multipart (JS does the same).
        let supports_octet = version_gte(&self.envd_version, ENVD_OCTET_STREAM_UPLOAD);
        let use_octet = (opts.use_octet_stream.unwrap_or(false) || opts.gzip) && supports_octet;

        let infos = if use_octet {
            let (body, encoding) = if opts.gzip {
                (gzip_bytes(&data).await?, Some("gzip"))
            } else {
                (data, None)
            };
            self.rest
                .post_files(
                    Some(path),
                    user.as_deref(),
                    reqwest::Body::from(body),
                    "application/octet-stream",
                    encoding,
                    &headers,
                )
                .await
                .map_err(|e| file_not_found_on_missing(e, path))?
        } else {
            let form = reqwest::multipart::Form::new().part(
                "file",
                reqwest::multipart::Part::bytes(data).file_name(path.to_string()),
            );
            self.rest
                .post_files_multipart(Some(path), user.as_deref(), form, &headers)
                .await
                .map_err(|e| file_not_found_on_missing(e, path))?
        };
        infos
            .into_iter()
            .next()
            .map(map_write_info)
            .ok_or_else(|| Error::Internal(format!("write to {path} returned no info")))
    }

    /// Write multiple files in one request. Always multipart (one `file` part per
    /// entry); `gzip` is ignored for the multi-file form.
    pub async fn write_files(
        &self,
        files: Vec<WriteEntry>,
        opts: FsWriteOpts,
    ) -> Result<Vec<WriteInfo>> {
        if files.is_empty() {
            return Ok(Vec::new());
        }
        // Always multipart (`gzip`/`use_octet_stream` do not apply to multi-file).
        let headers = self.write_headers(&opts)?;
        let user = self.resolve_user(opts.user.as_deref());
        let mut form = reqwest::multipart::Form::new();
        for entry in files {
            form = form.part(
                "file",
                reqwest::multipart::Part::bytes(entry.data).file_name(entry.path),
            );
        }
        let infos = self
            .rest
            .post_files_multipart(None, user.as_deref(), form, &headers)
            .await
            .map_err(|e| file_not_found_on_missing(e, "(batch write)"))?;
        Ok(infos.into_iter().map(map_write_info).collect())
    }
}

/// Gzip-compress a byte buffer.
async fn gzip_bytes(data: &[u8]) -> Result<Vec<u8>> {
    use async_compression::tokio::write::GzipEncoder;
    use tokio::io::AsyncWriteExt as _;
    let mut encoder = GzipEncoder::new(Vec::new());
    encoder
        .write_all(data)
        .await
        .map_err(|e| Error::Internal(format!("gzip failed: {e}")))?;
    encoder
        .shutdown()
        .await
        .map_err(|e| Error::Internal(format!("gzip finalize failed: {e}")))?;
    Ok(encoder.into_inner())
}
