//! Bearer-authenticated HTTP client for the E2B volume content API.
//!
//! Mirrors the structure of [`crate::api::client::ApiClient`] and
//! [`crate::envd::rest::EnvdApiClient`] but authenticates with
//! `Authorization: Bearer <token>` instead of `X-API-KEY`.

use std::time::Duration;

use bytes::Bytes;
use futures::{Stream, StreamExt as _};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Serialize, de::DeserializeOwned};

use crate::errors::{Error, Result};
use crate::volume::schema::Error as VolumeError;

/// Default timeout for file read/write operations (1 hour).
///
/// Used by [`super::control::Volume`] when building a content client for file
/// I/O methods (`read_file`, `read_file_bytes`, `read_file_stream`, `write_file`).
pub(crate) const FILE_TIMEOUT_MS: u64 = 3_600_000;

/// HTTP client for the E2B volume content API, authenticated with a short-lived
/// Bearer token obtained from the control-plane API.
///
/// The token is baked into the client's default headers so every request is
/// automatically authenticated.
pub(crate) struct VolumeApiClient {
    /// Underlying reqwest client with `Authorization: Bearer` baked in.
    http: reqwest::Client,
    /// Base URL (scheme + host, no trailing slash).
    base_url: String,
    /// Timeout in milliseconds for file read/write operations.
    file_timeout_ms: u64,
}

impl VolumeApiClient {
    /// Build a new client.
    ///
    /// `api_url` is the volume-content base URL (e.g. `https://volumes.e2b.app`).
    /// `token` is the short-lived Bearer token from the control-plane API.
    /// `request_timeout_ms` is applied to JSON API requests as a default client
    /// timeout; file operations always use the internal 1-hour `file_timeout_ms`.
    /// `proxy` is an optional HTTP/HTTPS proxy URL.
    pub(crate) fn new(
        api_url: &str,
        token: &str,
        request_timeout_ms: u64,
        proxy: Option<&str>,
    ) -> Result<VolumeApiClient> {
        let bearer = format!("Bearer {token}");
        let auth_value = HeaderValue::from_str(&bearer).map_err(|_| {
            Error::InvalidArgument(
                "volume token contains characters that are invalid in an HTTP header value"
                    .to_string(),
            )
        })?;
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, auth_value);

        let mut builder = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_millis(request_timeout_ms));

        if let Some(proxy_url) = proxy {
            let p = reqwest::Proxy::all(proxy_url).map_err(|e| {
                Error::InvalidArgument(format!("invalid proxy URL {proxy_url:?}: {e}"))
            })?;
            builder = builder.proxy(p);
        }
        let http = builder.build()?;

        Ok(VolumeApiClient {
            http,
            base_url: api_url.trim_end_matches('/').to_string(),
            file_timeout_ms: FILE_TIMEOUT_MS,
        })
    }

    /// Consume a non-2xx response and return a typed [`Error`].
    ///
    /// Tries to decode the `volume::schema::Error` JSON body for the error
    /// message; falls back to the HTTP reason phrase.
    async fn map_error_response(resp: reqwest::Response) -> Error {
        let status = resp.status();
        let body_bytes = resp.bytes().await.unwrap_or_default();
        let message = serde_json::from_slice::<VolumeError>(&body_bytes)
            .map(|e| e.message)
            .unwrap_or_else(|_| {
                status
                    .canonical_reason()
                    .unwrap_or("request failed")
                    .to_string()
            });
        Error::from_status(status.as_u16(), message)
    }

    /// Send a request and discard the response body on success.
    ///
    /// Useful for `DELETE` and other 204-no-content endpoints where the
    /// success response carries no JSON body. Mirrors [`crate::api::client::ApiClient::request_unit`].
    pub(crate) async fn request_unit<B>(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&B>,
    ) -> Result<()>
    where
        B: Serialize + ?Sized,
    {
        let url = format!("{}{path}", self.base_url);
        let mut req = self.http.request(method, &url);
        if !query.is_empty() {
            req = req.query(query);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?;
        if resp.status().is_success() {
            return Ok(());
        }
        Err(Self::map_error_response(resp).await)
    }

    /// Send a request and decode the JSON response body as `T`.
    ///
    /// An optional serializable `body` is sent as JSON. Non-2xx responses are
    /// mapped to typed errors via [`Error::from_status`].
    pub(crate) async fn request_json<T, B>(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&B>,
    ) -> Result<T>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        let url = format!("{}{path}", self.base_url);
        let mut req = self.http.request(method, &url);
        if !query.is_empty() {
            req = req.query(query);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?;
        if resp.status().is_success() {
            let bytes = resp.bytes().await?;
            return serde_json::from_slice::<T>(&bytes).map_err(|e| {
                Error::Internal(format!("failed to decode response from {path}: {e}"))
            });
        }
        Err(Self::map_error_response(resp).await)
    }

    /// Download the raw bytes at `path`, using the 1-hour file timeout.
    pub(crate) async fn read_bytes(&self, path: &str, query: &[(&str, String)]) -> Result<Vec<u8>> {
        let url = format!("{}{path}", self.base_url);
        let mut req = self
            .http
            .get(&url)
            .timeout(Duration::from_millis(self.file_timeout_ms));
        if !query.is_empty() {
            req = req.query(query);
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(Self::map_error_response(resp).await);
        }
        let bytes = resp.bytes().await?;
        Ok(bytes.to_vec())
    }

    /// Stream the bytes at `path`.
    ///
    /// `idle_timeout_ms` sets the overall request timeout; a value of `0` falls
    /// back to the client's 1-hour `file_timeout_ms`. reqwest's `gzip` feature
    /// transparently decompresses responses.
    pub(crate) async fn read_stream(
        &self,
        path: &str,
        query: &[(&str, String)],
        idle_timeout_ms: u64,
    ) -> Result<impl Stream<Item = Result<Bytes>> + use<>> {
        let timeout_ms = if idle_timeout_ms > 0 {
            idle_timeout_ms
        } else {
            self.file_timeout_ms
        };
        let url = format!("{}{path}", self.base_url);
        let mut req = self
            .http
            .get(&url)
            .timeout(Duration::from_millis(timeout_ms));
        if !query.is_empty() {
            req = req.query(query);
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(Self::map_error_response(resp).await);
        }
        Ok(resp.bytes_stream().map(|chunk| chunk.map_err(Error::from)))
    }

    /// PUT `body` as `application/octet-stream` to `path` and decode the JSON
    /// response as `T`. Uses the 1-hour file timeout.
    pub(crate) async fn write_bytes<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        body: Vec<u8>,
    ) -> Result<T> {
        let url = format!("{}{path}", self.base_url);
        let mut req = self
            .http
            .put(&url)
            .timeout(Duration::from_millis(self.file_timeout_ms))
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(body);
        if !query.is_empty() {
            req = req.query(query);
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(Self::map_error_response(resp).await);
        }
        let bytes = resp.bytes().await?;
        serde_json::from_slice::<T>(&bytes).map_err(|e| {
            Error::Internal(format!("failed to decode write response from {path}: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client_for(server: &MockServer) -> VolumeApiClient {
        VolumeApiClient::new(&server.uri(), "tkn", 5_000, None).expect("construct VolumeApiClient")
    }

    /// Verify that `read_bytes` issues a `GET` with the correct path, query
    /// param, and `Authorization: Bearer` header, and returns the body bytes.
    #[tokio::test]
    async fn read_bytes_gets_file() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumecontent/vol_1/file"))
            .and(query_param("path", "/a.txt"))
            .and(header("Authorization", "Bearer tkn"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"hello" as &[u8])
                    .append_header("Content-Type", "application/octet-stream"),
            )
            .mount(&server)
            .await;

        let client = client_for(&server);
        let bytes = client
            .read_bytes(
                "/volumecontent/vol_1/file",
                &[("path", "/a.txt".to_string())],
            )
            .await
            .expect("read_bytes ok");
        assert_eq!(bytes, b"hello");
    }

    /// Verify that `request_json` decodes an honest `VolumeEntryStat` wire JSON
    /// (lowercase `"type"` field) and that `from_wire` maps it to `File`.
    #[tokio::test]
    async fn request_json_decodes_entry_stat() {
        let server = MockServer::start().await;
        // This JSON is what the real API returns: integer timestamps are not used;
        // the wire format is RFC 3339. `"type"` is lowercase per the schema enum.
        let wire_json = serde_json::json!({
            "name": "file.txt",
            "path": "/a",
            "size": 42_i64,
            "mode": 420_u32,
            "uid": 1000_u32,
            "gid": 0_u32,
            "type": "file",
            "atime": "2024-01-01T00:00:00Z",
            "mtime": "2024-01-01T00:00:00Z",
            "ctime": "2024-01-01T00:00:00Z"
        });

        Mock::given(method("GET"))
            .and(path("/volumecontent/vol_1/path"))
            .and(query_param("path", "/a"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&wire_json))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let stat: crate::volume::schema::VolumeEntryStat = client
            .request_json(
                reqwest::Method::GET,
                "/volumecontent/vol_1/path",
                &[("path", "/a".to_string())],
                None::<&serde_json::Value>,
            )
            .await
            .expect("request_json ok");
        let mapped = crate::volume::types::VolumeEntryStat::from_wire(stat);
        assert_eq!(mapped.file_type, crate::volume::types::VolumeFileType::File,);
    }

    /// Verify that a 404 response with a `{"code":"not_found","message":"missing"}`
    /// body is mapped to `Error::NotFound`.
    #[tokio::test]
    async fn error_body_maps_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumecontent/vol_1/file"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_json(serde_json::json!({"code": "not_found", "message": "missing"})),
            )
            .mount(&server)
            .await;

        let client = client_for(&server);
        let result = client
            .request_json::<serde_json::Value, serde_json::Value>(
                reqwest::Method::GET,
                "/volumecontent/vol_1/file",
                &[],
                None,
            )
            .await;
        assert!(
            matches!(result, Err(crate::errors::Error::NotFound(_))),
            "expected NotFound, got {result:?}",
        );
    }
}
