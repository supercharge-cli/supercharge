//! Connect-over-JSON client: unary + (Task 5) server-streaming calls.

use std::sync::Arc;
use std::time::Duration;

use base64::prelude::{BASE64_STANDARD, Engine as _};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::connect::error::{map_code_to_error, parse_connect_error};
use crate::envd::versions::{ENVD_DEFAULT_USER, version_gte};
use crate::errors::{Error, Result};
use crate::logs::Logger;

/// Default sandbox user for Basic auth (matches `defaultUsername`).
const DEFAULT_USER: &str = "user";

/// Options for constructing a [`ConnectClient`].
#[allow(dead_code)] // used by Plan 3
pub(crate) struct ConnectClientOpts {
    /// Base URL of the sandbox envd RPC surface.
    pub base_url: String,
    /// envd access token (`X-Access-Token`).
    pub access_token: Option<String>,
    /// Sandbox id (`E2b-Sandbox-Id`).
    pub sandbox_id: String,
    /// envd port (`E2b-Sandbox-Port`).
    pub envd_port: u16,
    /// `User-Agent` header value.
    pub user_agent: String,
    /// envd version (gates the auth header).
    pub envd_version: String,
    /// Per-request timeout (ms).
    pub request_timeout_ms: u64,
    /// Optional logger.
    pub logger: Option<Arc<dyn Logger>>,
    /// Optional proxy URL.
    pub proxy: Option<String>,
}

/// Build the `Authorization: Basic base64("{user}:")` header, version-gated by
/// `ENVD_DEFAULT_USER` (0.4.0). Mirrors `authenticationHeader` from the JS SDK:
///
/// - Explicit `user` → always sends `Basic base64("{user}:")`.
/// - `user == None`, envd **< 0.4.0** → sends `Basic base64("user:")` because
///   old envd has no server-side default user and the client must specify one.
/// - `user == None`, envd **>= 0.4.0** → returns `None` (no header); the server
///   applies the default user itself.
#[allow(dead_code)] // used by Plan 3
pub(crate) fn auth_header(
    envd_version: &str,
    user: Option<&str>,
) -> Option<(HeaderName, HeaderValue)> {
    let username = match (user, version_gte(envd_version, ENVD_DEFAULT_USER)) {
        (Some(u), _) => u,
        (None, false) => DEFAULT_USER, // envd < 0.4.0 lacks the server default → send "user"
        (None, true) => return None,   // envd >= 0.4.0 → rely on the server default (no header)
    };
    let value = format!("Basic {}", BASE64_STANDARD.encode(format!("{username}:")));
    let header = HeaderValue::from_str(&value).ok()?;
    Some((reqwest::header::AUTHORIZATION, header))
}

/// Connect-over-JSON RPC client for the envd daemon.
#[allow(dead_code)] // used by Plan 3
pub(crate) struct ConnectClient {
    http: reqwest::Client,
    base_url: String,
    envd_version: String,
    request_timeout: Duration,
    logger: Option<Arc<dyn Logger>>,
}

#[allow(dead_code)] // methods used by Plan 3 callers
impl ConnectClient {
    /// Build the client, baking the sandbox/access/user-agent headers into the
    /// underlying `reqwest::Client` as default headers.
    pub(crate) fn new(opts: ConnectClientOpts) -> Result<Self> {
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

        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(opts.request_timeout_ms))
            .default_headers(headers);
        if let Some(proxy) = &opts.proxy {
            let p = reqwest::Proxy::all(proxy)
                .map_err(|e| Error::InvalidArgument(format!("invalid proxy URL {proxy:?}: {e}")))?;
            builder = builder.proxy(p);
        }
        let http = builder.build()?;

        Ok(Self {
            http,
            base_url: opts.base_url.trim_end_matches('/').to_string(),
            envd_version: opts.envd_version,
            request_timeout: Duration::from_millis(opts.request_timeout_ms),
            logger: opts.logger,
        })
    }

    /// Make a unary Connect call: `POST {base}{path}` with `application/json`,
    /// returning the decoded `Resp`. Maps a non-2xx response to a typed error.
    pub(crate) async fn unary<Req: Serialize, Resp: DeserializeOwned>(
        &self,
        path: &str,
        req: &Req,
        user: Option<&str>,
    ) -> Result<Resp> {
        let url = format!("{}{path}", self.base_url);
        if let Some(logger) = &self.logger {
            logger.debug(&format!("POST {url}"));
        }
        let body = serde_json::to_vec(req)
            .map_err(|e| Error::Internal(format!("failed to encode request for {path}: {e}")))?;

        let mut rb = self
            .http
            .post(&url)
            .timeout(self.request_timeout)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("connect-protocol-version", "1")
            .body(body);
        if let Some((name, value)) = auth_header(&self.envd_version, user) {
            rb = rb.header(name, value);
        }

        let resp = rb.send().await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if !status.is_success() {
            let (code, message) = parse_connect_error(status.as_u16(), &bytes);
            return Err(map_code_to_error(code, message));
        }
        serde_json::from_slice::<Resp>(&bytes)
            .map_err(|e| Error::Internal(format!("failed to decode response from {path}: {e}")))
    }

    /// Make a server-streaming Connect call: `POST {base}{path}` with
    /// `application/connect+json` and a single enveloped request; decode the
    /// response envelope stream into messages. The end-stream frame ends the
    /// stream (or yields a final `Err` if it carries an error).
    #[allow(dead_code)] // consumed by Plan 3 callers
    pub(crate) async fn server_stream<Req: Serialize, Resp: DeserializeOwned + 'static>(
        &self,
        path: &str,
        req: &Req,
        user: Option<&str>,
    ) -> Result<impl futures::Stream<Item = Result<Resp>> + use<Req, Resp>> {
        use crate::connect::envelope::{EnvelopeDecoder, encode_envelope};
        use futures::StreamExt as _;

        let url = format!("{}{path}", self.base_url);
        if let Some(logger) = &self.logger {
            logger.debug(&format!("POST {url} (stream)"));
        }
        let encoded = serde_json::to_vec(req)
            .map_err(|e| Error::Internal(format!("failed to encode request for {path}: {e}")))?;
        let body = encode_envelope(0, &encoded);

        let mut rb = self
            .http
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/connect+json")
            .header("connect-protocol-version", "1")
            .body(body);
        if let Some((name, value)) = auth_header(&self.envd_version, user) {
            rb = rb.header(name, value);
        }

        let resp = rb.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let bytes = resp.bytes().await?;
            let (code, message) = parse_connect_error(status.as_u16(), &bytes);
            return Err(map_code_to_error(code, message));
        }

        let path = path.to_string();
        let mut bytes_stream = resp.bytes_stream();
        let stream = async_stream::try_stream! {
            let mut decoder = EnvelopeDecoder::new();
            while let Some(chunk) = bytes_stream.next().await {
                let chunk = chunk?; // reqwest::Error -> Error::Transport
                decoder.push(&chunk);
                while let Some(frame) = decoder.next_frame() {
                    if frame.is_end_stream() {
                        // End-of-stream: payload may carry `{ "error": {code, message} }`.
                        if let Some(err) = end_stream_error(&frame.data) {
                            Err(err)?;
                        }
                        return;
                    }
                    let msg: Resp = serde_json::from_slice(&frame.data)
                        .map_err(|e| Error::Internal(format!("failed to decode stream frame from {path}: {e}")))?;
                    yield msg;
                }
            }
        };
        Ok(stream)
    }
}

use crate::errors::format_sandbox_timeout_error;

/// Refine an RPC error using a sandbox health probe. If `err` is a transport
/// error and the probe reports the sandbox responded unhealthy (`Some(false)`,
/// i.e. confirmed killed/timed-out), convert it to a sandbox-timeout
/// [`Error::Timeout`]; otherwise (transient `None`, or a non-transport error)
/// return `err` unchanged. Mirrors `handleRpcErrorWithHealthCheck`.
#[allow(dead_code)] // consumed by Plan 3
pub(crate) async fn handle_rpc_error<F, Fut>(err: Error, check_health: F) -> Error
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Option<bool>>,
{
    if matches!(err, Error::Transport(_))
        && let Some(false) = check_health().await
    {
        return format_sandbox_timeout_error("the sandbox is no longer running");
    }
    err
}

/// Parse a Connect end-of-stream frame; returns an [`Error`] if it carries one.
fn end_stream_error(data: &[u8]) -> Option<Error> {
    #[derive(serde::Deserialize)]
    struct EndStream {
        error: Option<serde_json::Value>,
    }
    let parsed = serde_json::from_slice::<EndStream>(data).ok()?;
    let err = parsed.error?;
    // The nested error is `{code, message}`; reuse the unary error parser.
    let bytes = serde_json::to_vec(&err).ok()?;
    let (code, message) = parse_connect_error(200, &bytes);
    Some(map_code_to_error(code, message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn opts_for(server: &MockServer) -> ConnectClientOpts {
        ConnectClientOpts {
            base_url: server.uri(),
            access_token: Some("tok".to_string()),
            sandbox_id: "sbx".to_string(),
            envd_port: 49983,
            user_agent: "e2b-rs/test".to_string(),
            envd_version: "0.6.0".to_string(),
            request_timeout_ms: 5_000,
            logger: None,
            proxy: None,
        }
    }

    #[test]
    fn auth_header_version_gating() {
        use base64::prelude::{BASE64_STANDARD, Engine as _};
        let basic = |u: &str| format!("Basic {}", BASE64_STANDARD.encode(format!("{u}:")));
        // Modern envd (>= 0.4.0), no explicit user → NO header (server default).
        assert!(auth_header("0.6.0", None).is_none());
        // Old envd (< 0.4.0), no explicit user → send the default user "user".
        let (name, value) = auth_header("0.3.0", None).expect("old envd sends default user");
        assert_eq!(name.as_str(), "authorization");
        assert_eq!(value.to_str().unwrap_or(""), basic("user")); // "Basic dXNlcjo="
        // Explicit user always sends, regardless of version.
        assert_eq!(
            auth_header("0.6.0", Some("root"))
                .expect("explicit")
                .1
                .to_str()
                .unwrap_or(""),
            basic("root")
        );
        assert_eq!(
            auth_header("0.3.0", Some("root"))
                .expect("explicit")
                .1
                .to_str()
                .unwrap_or(""),
            basic("root")
        );
    }

    #[tokio::test]
    async fn unary_posts_json_and_decodes_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/process.Process/List"))
            .and(header("content-type", "application/json"))
            .and(header("connect-protocol-version", "1"))
            .and(header("X-Access-Token", "tok"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"echoed": true})),
            )
            .mount(&server)
            .await;

        let client = ConnectClient::new(opts_for(&server)).expect("client");
        let req = serde_json::json!({"ping": 1});
        let resp: serde_json::Value = client
            .unary(super::super::PROC_LIST, &req, None)
            .await
            .expect("unary ok");
        assert_eq!(resp["echoed"], serde_json::Value::Bool(true));
    }

    #[tokio::test]
    async fn unary_maps_connect_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/filesystem.Filesystem/Stat"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "code": "not_found", "message": "no such file"
            })))
            .mount(&server)
            .await;
        let client = ConnectClient::new(opts_for(&server)).expect("client");
        let err = client
            .unary::<_, serde_json::Value>(super::super::FS_STAT, &serde_json::json!({}), None)
            .await
            .unwrap_err();
        match err {
            crate::errors::Error::NotFound(m) => assert!(m.contains("no such file")),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn server_stream_decodes_enveloped_messages_until_end() {
        use crate::connect::envelope::{FLAG_END_STREAM, encode_envelope};
        use futures::StreamExt as _;

        // Build a Connect streaming body: two message frames + a clean end-stream frame.
        let mut body = encode_envelope(0, br#"{"n":1}"#);
        body.extend(encode_envelope(0, br#"{"n":2}"#));
        body.extend(encode_envelope(FLAG_END_STREAM, b"{}"));

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/process.Process/Start"))
            .and(header("content-type", "application/connect+json"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        let client = ConnectClient::new(opts_for(&server)).expect("client");
        let stream = client
            .server_stream::<_, serde_json::Value>(
                super::super::PROC_START,
                &serde_json::json!({}),
                None,
            )
            .await
            .expect("stream opened");
        futures::pin_mut!(stream);
        let mut ns = Vec::new();
        while let Some(item) = stream.next().await {
            ns.push(item.expect("frame ok")["n"].as_i64().unwrap_or(0));
        }
        assert_eq!(ns, vec![1, 2]);
    }

    #[tokio::test]
    async fn handle_rpc_error_converts_transport_error_when_sandbox_dead() {
        // A transport error + health probe says "responded unhealthy" (Some(false)) → Timeout.
        let transport_err = {
            let e = reqwest::get("http://127.0.0.1:1/x").await.unwrap_err();
            crate::errors::Error::Transport(e)
        };
        let out = handle_rpc_error(transport_err, || async { Some(false) }).await;
        assert!(matches!(out, crate::errors::Error::Timeout(_)));

        // Same transport error but health unknown (None) → unchanged.
        let transport_err2 = {
            let e = reqwest::get("http://127.0.0.1:1/x").await.unwrap_err();
            crate::errors::Error::Transport(e)
        };
        let out2 = handle_rpc_error(transport_err2, || async { None }).await;
        assert!(matches!(out2, crate::errors::Error::Transport(_)));

        // A non-transport error is returned unchanged without probing.
        let out3 = handle_rpc_error(crate::errors::Error::NotFound("x".into()), || async {
            panic!("should not probe");
        })
        .await;
        assert!(matches!(out3, crate::errors::Error::NotFound(_)));
    }

    #[tokio::test]
    async fn server_stream_surfaces_end_stream_error() {
        use crate::connect::envelope::{FLAG_END_STREAM, encode_envelope};
        use futures::StreamExt as _;

        let mut body = encode_envelope(0, br#"{"n":1}"#);
        body.extend(encode_envelope(
            FLAG_END_STREAM,
            br#"{"error":{"code":"not_found","message":"gone"}}"#,
        ));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/process.Process/Start"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;
        let client = ConnectClient::new(opts_for(&server)).expect("client");
        let stream = client
            .server_stream::<_, serde_json::Value>(
                super::super::PROC_START,
                &serde_json::json!({}),
                None,
            )
            .await
            .expect("stream opened");
        futures::pin_mut!(stream);
        // First item: the data frame {"n":1} (Ok).
        let first = stream.next().await.expect("first item").expect("first ok");
        assert_eq!(first["n"].as_i64().unwrap_or(0), 1);
        // Second item: the end-stream error → Err(NotFound).
        let second = stream.next().await.expect("second item");
        assert!(matches!(second, Err(crate::errors::Error::NotFound(_))));
        // Stream ends after the error frame.
        assert!(stream.next().await.is_none());
    }
}
