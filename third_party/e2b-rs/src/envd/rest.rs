//! envd daemon REST client (port of the client side of `envd/api.ts`).

use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::errors::Result;
use crate::logs::Logger;

/// Health-check timeout for envd, matching `checkSandboxHealth` (5 s).
const HEALTH_TIMEOUT_MS: u64 = 5_000;

/// One element of the `POST /files` JSON response.
#[derive(serde::Deserialize)]
pub(crate) struct WriteInfoWire {
    /// Base name of the written file.
    pub name: String,
    /// File type string (e.g. `"FILE_TYPE_FILE"`).
    #[serde(rename = "type")]
    pub type_: Option<String>,
    /// Absolute path of the written file.
    pub path: String,
    /// Metadata persisted on the file, if any.
    pub metadata: Option<std::collections::HashMap<String, String>>,
}

/// Options for constructing an [`EnvdApiClient`].
#[allow(dead_code)] // used by Plan 3
pub(crate) struct EnvdApiClientOpts {
    /// Base URL of the sandbox's envd REST surface.
    pub base_url: String,
    /// envd access token (sent as `X-Access-Token`).
    pub access_token: Option<String>,
    /// Sandbox id (sent as `E2b-Sandbox-Id`).
    pub sandbox_id: String,
    /// envd port (sent as `E2b-Sandbox-Port`).
    pub envd_port: u16,
    /// `User-Agent` header value.
    pub user_agent: String,
    /// Per-request timeout in milliseconds.
    pub request_timeout_ms: u64,
    /// Optional logger.
    pub logger: Option<Arc<dyn Logger>>,
    /// Optional proxy URL.
    pub proxy: Option<String>,
}

/// Client for the in-sandbox envd daemon's REST surface (`/health`, and — in a
/// later milestone — `/files`).
#[allow(dead_code)] // used by Plan 3
pub(crate) struct EnvdApiClient {
    http: reqwest::Client,
    base_url: String,
    request_timeout: Duration,
    logger: Option<Arc<dyn Logger>>,
}

#[allow(dead_code)] // methods used by Plan 3 callers
impl EnvdApiClient {
    /// Build the client, baking the sandbox/access headers into the underlying
    /// `reqwest::Client` as default headers.
    pub(crate) fn new(opts: EnvdApiClientOpts) -> Result<Self> {
        let mut headers = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&opts.user_agent) {
            headers.insert(HeaderName::from_static("user-agent"), v);
        }
        if let Ok(v) = HeaderValue::from_str(&opts.sandbox_id) {
            headers.insert(HeaderName::from_static("e2b-sandbox-id"), v);
        }
        if let Ok(v) = HeaderValue::from_str(&opts.envd_port.to_string()) {
            headers.insert(HeaderName::from_static("e2b-sandbox-port"), v);
        }
        if let Some(token) = &opts.access_token
            && let Ok(v) = HeaderValue::from_str(token)
        {
            headers.insert(HeaderName::from_static("x-access-token"), v);
        }

        let mut builder = reqwest::Client::builder().default_headers(headers);
        if let Some(proxy) = &opts.proxy {
            let p = reqwest::Proxy::all(proxy).map_err(|e| {
                crate::errors::Error::InvalidArgument(format!("invalid proxy URL {proxy:?}: {e}"))
            })?;
            builder = builder.proxy(p);
        }
        let http = builder.build()?;

        Ok(Self {
            http,
            base_url: opts.base_url.trim_end_matches('/').to_string(),
            request_timeout: Duration::from_millis(opts.request_timeout_ms),
            logger: opts.logger,
        })
    }

    /// `GET {base}/files?path&username`. Maps non-2xx statuses to an error via
    /// `Error::from_status` (404 → `NotFound`; the `Filesystem` wrappers remap
    /// that to `FileNotFound`); returns the streaming response on success. With
    /// `gzip`, sends `Accept-Encoding: gzip` (reqwest's `gzip` feature
    /// transparently decompresses the response).
    pub(crate) async fn get_files(
        &self,
        path: &str,
        user: Option<&str>,
        gzip: bool,
    ) -> Result<reqwest::Response> {
        let url = format!("{}/files", self.base_url);
        let mut query: Vec<(&str, String)> = vec![("path", path.to_string())];
        if let Some(user) = user {
            query.push(("username", user.to_string()));
        }
        let mut rb = self
            .http
            .get(&url)
            .timeout(self.request_timeout)
            .query(&query);
        if gzip {
            rb = rb.header(reqwest::header::ACCEPT_ENCODING, "gzip");
        }
        let resp = rb.send().await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body = resp.text().await.unwrap_or_default();
        Err(crate::errors::Error::from_status(status.as_u16(), &body))
    }

    /// `POST {base}/files` with a prepared body + content-type. `path` is sent as
    /// a query param for single-file writes (omitted for multi-file). Maps non-2xx
    /// to an error; parses the `WriteInfo[]` JSON response.
    pub(crate) async fn post_files(
        &self,
        path: Option<&str>,
        user: Option<&str>,
        body: reqwest::Body,
        content_type: &str,
        content_encoding: Option<&str>,
        metadata_headers: &[(String, String)],
    ) -> crate::errors::Result<Vec<WriteInfoWire>> {
        let url = format!("{}/files", self.base_url);
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(path) = path {
            query.push(("path", path.to_string()));
        }
        if let Some(user) = user {
            query.push(("username", user.to_string()));
        }
        let mut rb = self
            .http
            .post(&url)
            .timeout(self.request_timeout)
            .query(&query)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(body);
        if let Some(enc) = content_encoding {
            rb = rb.header(reqwest::header::CONTENT_ENCODING, enc);
        }
        for (name, value) in metadata_headers {
            rb = rb.header(name.as_str(), value.as_str());
        }
        let resp = rb.send().await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(crate::errors::Error::from_status(status.as_u16(), &msg));
        }
        serde_json::from_slice::<Vec<WriteInfoWire>>(&bytes).map_err(|e| {
            crate::errors::Error::Internal(format!("failed to decode write response: {e}"))
        })
    }

    /// `POST {base}/files` as `multipart/form-data` (reqwest sets the
    /// content-type + boundary). Used for the multipart write path. `path` is
    /// sent as a query for single-file writes (the filename in the form part
    /// also carries it); omitted for multi-file writes.
    pub(crate) async fn post_files_multipart(
        &self,
        path: Option<&str>,
        user: Option<&str>,
        form: reqwest::multipart::Form,
        metadata_headers: &[(String, String)],
    ) -> crate::errors::Result<Vec<WriteInfoWire>> {
        let url = format!("{}/files", self.base_url);
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(path) = path {
            query.push(("path", path.to_string()));
        }
        if let Some(user) = user {
            query.push(("username", user.to_string()));
        }
        let mut rb = self
            .http
            .post(&url)
            .timeout(self.request_timeout)
            .query(&query)
            .multipart(form);
        for (name, value) in metadata_headers {
            rb = rb.header(name.as_str(), value.as_str());
        }
        let resp = rb.send().await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(crate::errors::Error::from_status(status.as_u16(), &msg));
        }
        serde_json::from_slice::<Vec<WriteInfoWire>>(&bytes).map_err(|e| {
            crate::errors::Error::Internal(format!("failed to decode write response: {e}"))
        })
    }

    /// Probe `GET /health` and return:
    ///
    /// - `Some(true)` — responded with a 2xx status (sandbox is healthy).
    /// - `Some(false)` — responded but with a non-2xx status (e.g. 502); Plan 3
    ///   treats this as a confirmed-dead sandbox.
    /// - `None` — the probe itself failed (transport error, timeout, DNS, etc.);
    ///   Plan 3 leaves transient errors intact and does not treat this as
    ///   conclusively unhealthy.
    ///
    /// Uses the fixed 5 s `HEALTH_TIMEOUT_MS` constant (matching JS
    /// `checkSandboxHealth`), not clamped to the per-request timeout.
    pub(crate) async fn check_health(&self) -> Option<bool> {
        let url = format!("{}/health", self.base_url);
        let timeout = Duration::from_millis(HEALTH_TIMEOUT_MS);
        if let Some(logger) = &self.logger {
            logger.debug(&format!("GET {url} (health)"));
        }
        match self.http.get(&url).timeout(timeout).send().await {
            Ok(resp) => Some(resp.status().is_success()),
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn opts_for(server: &MockServer) -> EnvdApiClientOpts {
        EnvdApiClientOpts {
            base_url: server.uri(),
            access_token: Some("tok-123".to_string()),
            sandbox_id: "sbx_test".to_string(),
            envd_port: 49983,
            user_agent: "e2b-rs/test".to_string(),
            request_timeout_ms: 5_000,
            logger: None,
            proxy: None,
        }
    }

    #[tokio::test]
    async fn check_health_true_on_200_with_headers() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .and(header("X-Access-Token", "tok-123"))
            .and(header("E2b-Sandbox-Id", "sbx_test"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let client = EnvdApiClient::new(opts_for(&server)).expect("construct");
        assert_eq!(client.check_health().await, Some(true));
    }

    #[tokio::test]
    async fn check_health_false_on_502() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;
        let client = EnvdApiClient::new(opts_for(&server)).expect("construct");
        assert_eq!(client.check_health().await, Some(false));
    }

    #[tokio::test]
    async fn check_health_none_on_connection_error() {
        // Port 1 is unroutable on all modern OSes; the connect should fail
        // immediately and the probe should return None (unknown / transport error).
        let client = EnvdApiClient::new(EnvdApiClientOpts {
            base_url: "http://127.0.0.1:1".to_string(),
            access_token: None,
            sandbox_id: "sbx_test".to_string(),
            envd_port: 49983,
            user_agent: "e2b-rs/test".to_string(),
            request_timeout_ms: 5_000,
            logger: None,
            proxy: None,
        })
        .expect("construct");
        assert_eq!(client.check_health().await, None);
    }
}
