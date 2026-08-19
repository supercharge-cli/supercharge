//! Control-plane REST client (port of `api/index.ts`).

use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::de::DeserializeOwned;

use crate::api::schema::Error as ApiError;
use crate::connection_config::ConnectionConfig;
use crate::errors::{Error, Result};
use crate::http::inflight::ConcurrencyLimiter;
use crate::logs::Logger;

/// Validate an E2B API key: `e2b_` followed by one or more lowercase hex chars.
/// Mirrors `API_KEY_PATTERN` in `api/index.ts`.
#[allow(dead_code)] // consumed by ApiClient::new and by Plan 3+ callers
pub(crate) fn validate_api_key(key: &str) -> Result<()> {
    let valid = key.strip_prefix("e2b_").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    });
    if valid {
        Ok(())
    } else {
        let example = format!("e2b_{}", "0".repeat(40));
        Err(Error::Authentication(format!(
            "Invalid API key format: expected \"e2b_\" followed by hex characters (e.g. \"{example}\")."
        )))
    }
}

/// Client for the E2B control-plane REST API.
#[allow(dead_code)] // fully used once Plan 3+ sandbox/template calls are wired
pub(crate) struct ApiClient {
    http: reqwest::Client,
    base_url: String,
    request_timeout: Duration,
    limiter: ConcurrencyLimiter,
    logger: Option<Arc<dyn Logger>>,
}

#[allow(dead_code)] // methods used by tests and by Plan 3+ callers
impl ApiClient {
    /// Build a client from a [`ConnectionConfig`]. Validates the API key format
    /// when present and `validate_api_key` is enabled; errors when
    /// `require_api_key` is set but no key is configured.
    pub(crate) fn new(config: &ConnectionConfig, require_api_key: bool) -> Result<Self> {
        if require_api_key && config.api_key.is_none() {
            return Err(Error::Authentication(
                "API key is required: set E2B_API_KEY or pass api_key in the options.".to_string(),
            ));
        }
        if let Some(key) = &config.api_key
            && config.validate_api_key
        {
            validate_api_key(key)?;
        }

        let mut headers = HeaderMap::new();
        for (name, value) in &config.headers {
            if let (Ok(n), Ok(v)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                headers.insert(n, v);
            }
        }
        if let Some(key) = &config.api_key
            && let Ok(v) = HeaderValue::from_str(key)
        {
            headers.insert(HeaderName::from_static("x-api-key"), v);
        }
        if let Some(token) = &config.access_token
            && let Ok(v) = HeaderValue::from_str(&format!("Bearer {token}"))
        {
            headers.insert(reqwest::header::AUTHORIZATION, v);
        }

        let mut builder = reqwest::Client::builder().default_headers(headers);
        if let Some(proxy) = &config.proxy {
            let p = reqwest::Proxy::all(proxy)
                .map_err(|e| Error::InvalidArgument(format!("invalid proxy URL {proxy:?}: {e}")))?;
            builder = builder.proxy(p);
        }
        let http = builder.build()?;

        Ok(Self {
            http,
            base_url: config.api_url.trim_end_matches('/').to_string(),
            request_timeout: Duration::from_millis(config.request_timeout_ms),
            limiter: ConcurrencyLimiter::new(config.api_inflight_requests),
            logger: config.logger.clone(),
        })
    }

    /// Make a request, deserializing a JSON response into `T`. Centralizes the
    /// in-flight cap, timeout, status→error mapping, and logging.
    pub(crate) async fn request<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<T> {
        let (bytes, _) = self.send(method, path, query, body).await?;
        serde_json::from_slice::<T>(&bytes)
            .map_err(|e| Error::Internal(format!("failed to decode response from {path}: {e}")))
    }

    /// Like [`ApiClient::request`] but discards the response body.
    pub(crate) async fn request_unit(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<()> {
        self.send(method, path, query, body).await.map(|_| ())
    }

    /// Like [`ApiClient::request`] but also returns the response headers (for
    /// pagination cursors).
    pub(crate) async fn request_with_headers<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<(T, reqwest::header::HeaderMap)> {
        let (bytes, headers) = self.send(method, path, query, body).await?;
        let value = serde_json::from_slice::<T>(&bytes)
            .map_err(|e| Error::Internal(format!("failed to decode response from {path}: {e}")))?;
        Ok((value, headers))
    }

    /// Health check: `GET /health`.
    pub(crate) async fn health(&self) -> Result<()> {
        self.request_unit(reqwest::Method::GET, "/health", &[], None)
            .await
    }

    /// Shared request execution: build, send, log, map status to `Error`, and
    /// return the raw success body bytes along with the response headers.
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<(Vec<u8>, reqwest::header::HeaderMap)> {
        let _permit = self.limiter.acquire().await;
        let url = format!("{}{path}", self.base_url);
        if let Some(logger) = &self.logger {
            logger.debug(&format!("{method} {url}"));
        }

        let mut req = self
            .http
            .request(method, &url)
            .timeout(self.request_timeout);
        if !query.is_empty() {
            req = req.query(query);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let body_bytes = resp.bytes().await?;

        if status.is_success() {
            return Ok((body_bytes.to_vec(), headers));
        }

        // Extract the server's error message (control-plane Error.message), else the status reason.
        let message = serde_json::from_slice::<ApiError>(&body_bytes)
            .map(|e| e.message)
            .unwrap_or_else(|_| {
                status
                    .canonical_reason()
                    .unwrap_or("request failed")
                    .to_string()
            });
        if let Some(logger) = &self.logger {
            logger.error(&format!("{} {url} -> {}", status.as_u16(), message));
        }
        Err(Error::from_status(status.as_u16(), message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection_config::{ConnectionConfig, ConnectionConfigOpts};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client_for(server: &MockServer) -> ApiClient {
        let config = ConnectionConfig::new(ConnectionConfigOpts {
            api_key: Some("e2b_0123456789abcdef".to_string()),
            api_url: Some(server.uri()),
            ..Default::default()
        });
        ApiClient::new(&config, true).expect("construct ApiClient")
    }

    #[test]
    fn validate_api_key_accepts_valid_and_rejects_invalid() {
        assert!(validate_api_key("e2b_0123456789abcdef").is_ok());
        assert!(matches!(
            validate_api_key("not-a-key"),
            Err(crate::errors::Error::Authentication(_))
        ));
        // Uppercase hex is NOT allowed (JS pattern is lowercase [0-9a-f]).
        assert!(validate_api_key("e2b_ABCDEF").is_err());
        assert!(validate_api_key("e2b_").is_err());
    }

    #[test]
    fn new_requires_api_key_when_asked() {
        let config = ConnectionConfig::new(ConnectionConfigOpts {
            api_key: None,
            api_url: Some("https://api.example".to_string()),
            ..Default::default()
        });
        assert!(matches!(
            ApiClient::new(&config, true),
            Err(crate::errors::Error::Authentication(_))
        ));
        // require_api_key=false allows construction without a key.
        assert!(ApiClient::new(&config, false).is_ok());
    }

    #[tokio::test]
    async fn health_sends_api_key_header_and_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .and(header("X-API-KEY", "e2b_0123456789abcdef"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        client_for(&server).health().await.expect("health ok");
    }

    #[tokio::test]
    async fn maps_status_codes_to_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "code": 401, "message": "bad key"
            })))
            .mount(&server)
            .await;
        let err = client_for(&server).health().await.unwrap_err();
        match err {
            crate::errors::Error::Authentication(msg) => assert!(msg.contains("bad key")),
            other => panic!("expected Authentication, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rate_limit_maps_to_rate_limit_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;
        assert!(matches!(
            client_for(&server).health().await,
            Err(crate::errors::Error::RateLimit(_))
        ));
    }
}
