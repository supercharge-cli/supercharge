//! Browser sign-in for the existing `mistral` provider.
//!
//! The state machine follows the console PKCE S256 sign-in API (start, poll,
//! one-shot exchange). This file is Supercharge code. It does not copy
//! third-party UI copy or an OIDC refresh-token client.
//!
//! Issued keys are returned only to the setup save path. Errors and `Debug`
//! output omit verifiers, exchange tokens, and API keys.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use crate::agent::provider_setup::{self, ProviderSetup, SetupWireFormat};

pub const MISTRAL_PROVIDER_ID: &str = "mistral";
pub const MISTRAL_AUTH_PROVIDER_ID: &str = "mistral-auth";
pub const BROWSER_BASE_URL: &str = "https://console.mistral.ai";
pub const AUTH_API_BASE_URL: &str = "https://console.mistral.ai/api";
/// Inference stays on the existing Chat Completions host. Sign-in does not.
pub const INFERENCE_BASE_URL: &str = "https://api.mistral.ai/v1";

const POLL_INTERVAL: Duration = Duration::from_secs(3);
/// A 429 on poll or exchange is a refusal, not a created key. Back off, then retry.
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(8);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Stop polling even if the server advertises a longer lifetime.
const MAX_SIGN_IN_LIFETIME: Duration = Duration::from_secs(15 * 60);
const MAX_CONSECUTIVE_POLL_FAILURES: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInCode {
    StartFailed,
    PollFailed,
    UnknownState,
    ExchangeFailed,
    ExchangeUncertain,
    MissingApiKey,
    MissingExchangeToken,
    Expired,
    Denied,
    ProviderError,
    TimedOut,
    Cancelled,
    RateLimited,
    UntrustedUrl,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SignInError {
    pub code: SignInCode,
    message: String,
}

impl SignInError {
    fn new(code: SignInCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> SignInCode {
        self.code
    }
}

impl std::fmt::Display for SignInError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::fmt::Debug for SignInError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignInError")
            .field("code", &self.code)
            .finish()
    }
}

impl std::error::Error for SignInError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpReply {
    pub status: u16,
    pub body: String,
}

#[derive(Debug)]
pub enum TransportError {
    Network,
    Timeout,
}

pub trait SignInTransport: Send {
    fn call(&self, method: &str, url: &str, json_body: Option<String>) -> Result<HttpReply, TransportError>;
}

pub struct SignInHooks {
    pub now: Box<dyn Fn() -> DateTime<Utc> + Send>,
    pub sleep: Box<dyn Fn(Duration) + Send>,
    pub cancelled: Box<dyn Fn() -> bool + Send>,
    pub poll_interval: Duration,
}

impl SignInHooks {
    pub fn virtual_clock(now: DateTime<Utc>) -> (Arc<Mutex<DateTime<Utc>>>, Self) {
        let clock = Arc::new(Mutex::new(now));
        let clock_now = Arc::clone(&clock);
        let clock_sleep = Arc::clone(&clock);
        let hooks = Self {
            now: Box::new(move || *clock_now.lock().unwrap_or_else(|e| e.into_inner())),
            sleep: Box::new(move |delay| {
                let mut guard = clock_sleep.lock().unwrap_or_else(|e| e.into_inner());
                *guard += chrono::Duration::from_std(delay).unwrap_or_else(|_| chrono::Duration::zero());
            }),
            cancelled: Box::new(|| false),
            poll_interval: Duration::from_millis(1),
        };
        (clock, hooks)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartedSignIn {
    pub process_id: String,
    pub sign_in_url: String,
    pub poll_url: String,
    pub expires_at: DateTime<Utc>,
}

/// RFC 7636 S256 challenge. The verifier is ASCII; the result is unpadded base64url.
pub fn code_challenge_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

pub fn generate_pkce_pair() -> (String, String) {
    let mut bytes = [0u8; 48];
    let file = std::fs::File::open("/dev/urandom").expect("urandom");
    let mut file = file;
    std::io::Read::read_exact(&mut file, &mut bytes).expect("urandom");
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = code_challenge_s256(&verifier);
    (verifier, challenge)
}

pub fn authenticate<T: SignInTransport>(
    transport: &T,
    hooks: &SignInHooks,
) -> Result<String, SignInError> {
    let (verifier, challenge) = generate_pkce_pair();
    let started = start_process(transport, &challenge)?;
    finish_attempt(transport, hooks, &started, &verifier)
}

fn finish_attempt<T: SignInTransport>(
    transport: &T,
    hooks: &SignInHooks,
    started: &StartedSignIn,
    verifier: &str,
) -> Result<String, SignInError> {
    let started_at = (hooks.now)();
    let cap = started_at + chrono::Duration::from_std(MAX_SIGN_IN_LIFETIME).unwrap_or(chrono::Duration::minutes(15));
    let deadline = started.expires_at.min(cap);
    let mut consecutive_poll_failures = 0u32;
    loop {
        if (hooks.cancelled)() {
            return Err(SignInError::new(
                SignInCode::Cancelled,
                "Browser sign-in was cancelled.",
            ));
        }
        let now = (hooks.now)();
        if now >= deadline {
            return Err(SignInError::new(
                SignInCode::TimedOut,
                "Browser sign-in timed out.",
            ));
        }
        match poll_once(transport, &started.poll_url) {
            Err(error)
                if error.code == SignInCode::PollFailed || error.code == SignInCode::RateLimited =>
            {
                consecutive_poll_failures += 1;
                if consecutive_poll_failures >= MAX_CONSECUTIVE_POLL_FAILURES {
                    return Err(error);
                }
                sleep_until(
                    hooks,
                    deadline,
                    error.code == SignInCode::RateLimited,
                )?;
                continue;
            }
            Err(error) => return Err(error),
            Ok(Poll::Pending) => {
                consecutive_poll_failures = 0;
                sleep_until(hooks, deadline, false)?;
            }
            Ok(Poll::Expired) => {
                return Err(SignInError::new(
                    SignInCode::Expired,
                    "Browser sign-in expired.",
                ));
            }
            Ok(Poll::Denied) => {
                return Err(SignInError::new(
                    SignInCode::Denied,
                    "Browser sign-in was denied.",
                ));
            }
            Ok(Poll::Error(message)) => {
                return Err(SignInError::new(SignInCode::ProviderError, message));
            }
            Ok(Poll::Completed(token)) => {
                if token.is_empty() {
                    return Err(SignInError::new(
                        SignInCode::MissingExchangeToken,
                        "Sign-in finished, but setup could not continue.",
                    ));
                }
                if (hooks.cancelled)() {
                    return Err(SignInError::new(
                        SignInCode::Cancelled,
                        "Browser sign-in was cancelled.",
                    ));
                }
                // One exchange. A transport failure after the request is sent
                // is uncertain: the key may already exist, so this is not retried.
                return exchange_once(transport, hooks, &started.process_id, &token, verifier);
            }
        }
    }
}

fn sleep_until(
    hooks: &SignInHooks,
    deadline: DateTime<Utc>,
    rate_limited: bool,
) -> Result<(), SignInError> {
    let remaining = deadline - (hooks.now)();
    if remaining <= chrono::Duration::zero() {
        return Err(SignInError::new(
            SignInCode::TimedOut,
            "Browser sign-in timed out.",
        ));
    }
    let remaining = remaining.to_std().unwrap_or(hooks.poll_interval);
    let pause = if rate_limited {
        hooks.poll_interval.max(RATE_LIMIT_BACKOFF)
    } else {
        hooks.poll_interval
    };
    (hooks.sleep)(pause.min(remaining));
    Ok(())
}

enum Poll {
    Pending,
    Completed(String),
    Expired,
    Denied,
    Error(String),
}

pub fn start_process<T: SignInTransport>(
    transport: &T,
    challenge: &str,
) -> Result<StartedSignIn, SignInError> {
    let url = pinned_join(AUTH_API_BASE_URL, "vibe/sign-in").map_err(|_| {
        SignInError::new(
            SignInCode::StartFailed,
            "Failed to start browser sign-in.",
        )
    })?;
    let body = serde_json::json!({
        "code_challenge": challenge,
        "code_challenge_method": "S256",
    })
    .to_string();
    let reply = transport.call("POST", &url, Some(body)).map_err(|error| {
        transport_error(SignInCode::StartFailed, "Failed to start browser sign-in.", error)
    })?;
    if reply.status == 429 {
        return Err(rate_limited());
    }
    if !(200..300).contains(&reply.status) {
        return Err(SignInError::new(
            SignInCode::StartFailed,
            format!("Failed to start browser sign-in (HTTP {}).", reply.status),
        ));
    }
    let payload: serde_json::Value = serde_json::from_str(&reply.body).map_err(|_| {
        SignInError::new(
            SignInCode::StartFailed,
            "Failed to start browser sign-in.",
        )
    })?;
    let process_id = payload
        .get("process_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let sign_in_url = payload
        .get("sign_in_url")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let poll_url = payload.get("poll_url").and_then(|v| v.as_str()).unwrap_or("");
    let expires_raw = payload
        .get("expires_at")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if process_id.is_empty() || sign_in_url.is_empty() || poll_url.is_empty() || expires_raw.is_empty()
    {
        return Err(SignInError::new(
            SignInCode::StartFailed,
            "Failed to start browser sign-in.",
        ));
    }
    if !process_id_ok(&process_id) {
        return Err(SignInError::new(
            SignInCode::StartFailed,
            "Failed to start browser sign-in.",
        ));
    }
    let sign_in_url = validate_url(sign_in_url, BROWSER_BASE_URL).map_err(|_| {
        SignInError::new(
            SignInCode::UntrustedUrl,
            "Browser sign-in returned an unexpected sign-in link.",
        )
    })?;
    let poll_url = validate_url(poll_url, AUTH_API_BASE_URL).map_err(|_| {
        SignInError::new(
            SignInCode::UntrustedUrl,
            "Browser sign-in returned an unexpected status link.",
        )
    })?;
    let expires_at = DateTime::parse_from_rfc3339(expires_raw)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| {
            SignInError::new(
                SignInCode::StartFailed,
                "Failed to start browser sign-in.",
            )
        })?;
    Ok(StartedSignIn {
        process_id,
        sign_in_url,
        poll_url,
        expires_at,
    })
}

fn poll_once<T: SignInTransport>(transport: &T, poll_url: &str) -> Result<Poll, SignInError> {
    let poll_url = validate_url(poll_url, AUTH_API_BASE_URL).map_err(|_| {
        SignInError::new(
            SignInCode::PollFailed,
            "Browser sign-in status could not be retrieved.",
        )
    })?;
    let reply = transport
        .call("GET", &poll_url, None)
        .map_err(|error| {
            transport_error(
                SignInCode::PollFailed,
                "Browser sign-in status could not be retrieved.",
                error,
            )
        })?;
    if reply.status == 410 {
        return Ok(Poll::Expired);
    }
    if reply.status == 429 {
        return Err(rate_limited());
    }
    if !(200..300).contains(&reply.status) {
        return Err(SignInError::new(
            SignInCode::PollFailed,
            "Browser sign-in status could not be retrieved.",
        ));
    }
    let payload: serde_json::Value = serde_json::from_str(&reply.body).map_err(|_| {
        SignInError::new(
            SignInCode::PollFailed,
            "Browser sign-in status could not be retrieved.",
        )
    })?;
    let status = payload.get("status").and_then(|v| v.as_str()).unwrap_or("");
    match status {
        "pending" => Ok(Poll::Pending),
        "completed" => Ok(Poll::Completed(
            payload
                .get("exchange_token")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned(),
        )),
        "expired" => Ok(Poll::Expired),
        "denied" => Ok(Poll::Denied),
        "error" => Ok(Poll::Error(safe_provider_message(
            payload.get("message").and_then(|v| v.as_str()),
        ))),
        _ => Err(SignInError::new(
            SignInCode::UnknownState,
            "Browser sign-in returned an unknown state.",
        )),
    }
}

fn exchange_once<T: SignInTransport>(
    transport: &T,
    hooks: &SignInHooks,
    process_id: &str,
    exchange_token: &str,
    verifier: &str,
) -> Result<String, SignInError> {
    if !process_id_ok(process_id) {
        return Err(SignInError::new(
            SignInCode::ExchangeFailed,
            "Failed to finish browser sign-in.",
        ));
    }
    let url = pinned_join(
        AUTH_API_BASE_URL,
        &format!("vibe/sign-in/{process_id}/exchange"),
    )
    .map_err(|_| {
        SignInError::new(
            SignInCode::ExchangeFailed,
            "Failed to finish browser sign-in.",
        )
    })?;
    let body = serde_json::json!({
        "exchange_token": exchange_token,
        "code_verifier": verifier,
    })
    .to_string();
    // 429 means the exchange was refused. The same token can be sent again.
    // A dropped response is different: the key may already exist, so that path
    // stays one-shot.
    let mut rate_limits = 0u32;
    let reply = loop {
        let reply = match transport.call("POST", &url, Some(body.clone())) {
            Ok(reply) => reply,
            Err(_) => {
                return Err(SignInError::new(
                    SignInCode::ExchangeUncertain,
                    "Browser sign-in could not be confirmed. It was not retried, so a second key was not created. Start sign-in again.",
                ));
            }
        };
        if reply.status != 429 {
            break reply;
        }
        rate_limits += 1;
        if rate_limits >= MAX_CONSECUTIVE_POLL_FAILURES {
            return Err(rate_limited());
        }
        let deadline = (hooks.now)()
            + chrono::Duration::from_std(RATE_LIMIT_BACKOFF).unwrap_or(chrono::Duration::seconds(8));
        sleep_until(hooks, deadline, true)?;
    };
    if reply.status == 403 && workspace_denied(&reply.body) {
        return Err(SignInError::new(
            SignInCode::ExchangeFailed,
            "This workspace cannot create an API key. Sign in again and choose another workspace, or ask an administrator.",
        ));
    }
    if !(200..300).contains(&reply.status) {
        return Err(SignInError::new(
            SignInCode::ExchangeFailed,
            format!("Failed to finish browser sign-in (HTTP {}).", reply.status),
        ));
    }
    let payload: serde_json::Value = serde_json::from_str(&reply.body).map_err(|_| {
        SignInError::new(
            SignInCode::ExchangeFailed,
            "Failed to finish browser sign-in.",
        )
    })?;
    match payload.get("api_key").and_then(|v| v.as_str()) {
        Some(key) if !key.trim().is_empty() => Ok(key.to_owned()),
        _ => Err(SignInError::new(
            SignInCode::MissingApiKey,
            "Browser sign-in finished without an API key.",
        )),
    }
}

fn workspace_denied(body: &str) -> bool {
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    payload.get("error_code").and_then(|v| v.as_i64()) == Some(9004)
}

fn rate_limited() -> SignInError {
    SignInError::new(
        SignInCode::RateLimited,
        "Browser sign-in was rate limited (HTTP 429). Wait, then try again.",
    )
}

fn transport_error(code: SignInCode, message: &str, error: TransportError) -> SignInError {
    let message = match error {
        TransportError::Timeout => format!("{message} The request timed out."),
        TransportError::Network => message.to_owned(),
    };
    SignInError::new(code, message)
}

fn safe_provider_message(raw: Option<&str>) -> String {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return "Browser sign-in failed.".to_owned();
    };
    if raw.len() > 180 || raw.contains("sk-") || raw.to_ascii_lowercase().contains("api_key") {
        return "Browser sign-in failed.".to_owned();
    }
    raw.to_owned()
}

fn process_id_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Join `path` onto `base` without dropping a final path segment.
/// `Url::join` against `https://host/api` (no trailing slash) resolves
/// `vibe/sign-in` to `https://host/vibe/sign-in`.
pub fn pinned_join(base: &str, path: &str) -> Result<String, ()> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() || base.contains(['\n', '\r', ' ']) {
        return Err(());
    }
    let rooted = format!("{base}/");
    let base_url = url::Url::parse(&rooted).map_err(|_| ())?;
    if base_url.username() != "" || base_url.password().is_some() {
        return Err(());
    }
    let path = path.trim().trim_start_matches('/');
    if path.is_empty() || path.contains(['\n', '\r', ' ']) || path.contains("://") {
        return Err(());
    }
    let joined = base_url.join(path).map_err(|_| ())?;
    if joined.scheme() != base_url.scheme() || joined.host_str() != base_url.host_str() {
        return Err(());
    }
    Ok(joined.to_string())
}

pub fn validate_url(value: &str, base: &str) -> Result<String, ()> {
    let url = url::Url::parse(value).map_err(|_| ())?;
    let base = url::Url::parse(base).map_err(|_| ())?;
    if url.scheme() != "https" || base.scheme() != "https" {
        return Err(());
    }
    if url.username() != "" || !url.password().is_none() {
        return Err(());
    }
    if !origin_eq(&url, &base) {
        return Err(());
    }
    if !path_under(url.path(), base.path()) {
        return Err(());
    }
    Ok(value.to_owned())
}

fn origin_eq(url: &url::Url, base: &url::Url) -> bool {
    host_key(url) == host_key(base) && effective_port(url) == effective_port(base)
}

fn host_key(url: &url::Url) -> String {
    url.host_str().unwrap_or("").trim_end_matches('.').to_ascii_lowercase()
}

fn effective_port(url: &url::Url) -> u16 {
    url.port().unwrap_or(443)
}

fn path_under(path: &str, base_path: &str) -> bool {
    let path = normalize_path(path);
    let base_path = normalize_path(base_path).trim_end_matches('/').to_owned();
    if base_path.is_empty() {
        return true;
    }
    path == base_path || path.starts_with(&(base_path + "/"))
}

fn normalize_path(path: &str) -> String {
    let decoded = percent_decode(path);
    let mut parts = Vec::new();
    for part in decoded.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            parts.pop();
            continue;
        }
        if part.contains('\\') {
            return "/".to_owned();
        }
        parts.push(part);
    }
    if parts.is_empty() {
        "/".to_owned()
    } else {
        format!("/{}", parts.join("/"))
    }
}

fn percent_decode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                16,
            ) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct ReqwestTransport {
    client: reqwest::blocking::Client,
}

impl SignInTransport for ReqwestTransport {
    fn call(
        &self,
        method: &str,
        url: &str,
        json_body: Option<String>,
    ) -> Result<HttpReply, TransportError> {
        // Redirects are refused so a verifier or exchange token cannot follow
        // a 3xx onto another origin.
        let builder = match method {
            "POST" => self.client.post(url),
            "GET" => self.client.get(url),
            _ => return Err(TransportError::Network),
        };
        let builder = if let Some(body) = json_body {
            builder
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
        } else {
            builder
        };
        match builder.send() {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().unwrap_or_default();
                Ok(HttpReply { status, body })
            }
            Err(error) if error.is_timeout() => Err(TransportError::Timeout),
            Err(_) => Err(TransportError::Network),
        }
    }
}

fn production_transport() -> Result<ReqwestTransport, SignInError> {
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent("Supercharge")
        .build()
        .map_err(|_| {
            SignInError::new(
                SignInCode::StartFailed,
                "Failed to start browser sign-in.",
            )
        })?;
    Ok(ReqwestTransport { client })
}

fn interruptible_sleep(cancel: &AtomicBool, delay: Duration) {
    let slice = Duration::from_millis(200);
    let mut left = delay;
    while !left.is_zero() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let step = slice.min(left);
        std::thread::sleep(step);
        left = left.saturating_sub(step);
    }
}

pub fn authenticate_production(cancel: &Arc<AtomicBool>) -> Result<String, SignInError> {
    let transport = production_transport()?;
    let sleep_cancel = Arc::clone(cancel);
    let check_cancel = Arc::clone(cancel);
    let hooks = SignInHooks {
        now: Box::new(Utc::now),
        sleep: Box::new(move |delay| interruptible_sleep(&sleep_cancel, delay)),
        cancelled: Box::new(move || check_cancel.load(Ordering::Relaxed)),
        poll_interval: POLL_INTERVAL,
    };
    authenticate(&transport, &hooks)
}

struct LiveSession {
    generation: u64,
    started: StartedSignIn,
    verifier: String,
    cancel: Arc<AtomicBool>,
    exchange_started: bool,
    api_key: Option<String>,
}

fn live_slot() -> &'static Mutex<Option<LiveSession>> {
    static SLOT: std::sync::OnceLock<Mutex<Option<LiveSession>>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn lock_slot() -> std::sync::MutexGuard<'static, Option<LiveSession>> {
    live_slot().lock().unwrap_or_else(|e| e.into_inner())
}

/// Start a sign-in and keep the verifier in process memory for this generation.
pub fn begin_live(generation: u64) -> Result<String, SignInError> {
    let transport = production_transport()?;
    let (verifier, challenge) = generate_pkce_pair();
    let started = start_process(&transport, &challenge)?;
    let url = started.sign_in_url.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    *lock_slot() = Some(LiveSession {
        generation,
        started,
        verifier,
        cancel,
        exchange_started: false,
        api_key: None,
    });
    Ok(url)
}

pub fn sign_in_url(generation: u64) -> Option<String> {
    lock_slot().as_ref().and_then(|session| {
        (session.generation == generation).then(|| session.started.sign_in_url.clone())
    })
}

pub fn cancel_live(generation: u64) {
    if let Some(session) = lock_slot().as_ref() {
        if session.generation == generation {
            session.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// Poll until the attempt finishes. The key stays in the session until [`take_api_key`].
pub fn wait_live(generation: u64) -> Result<(), SignInError> {
    let (started, verifier, cancel) = {
        let guard = lock_slot();
        let Some(session) = guard.as_ref() else {
            return Err(SignInError::new(
                SignInCode::StartFailed,
                "Browser sign-in is not in progress.",
            ));
        };
        if session.generation != generation {
            return Err(SignInError::new(
                SignInCode::Cancelled,
                "Browser sign-in was cancelled.",
            ));
        }
        if session.exchange_started {
            return Err(SignInError::new(
                SignInCode::ExchangeUncertain,
                "Browser sign-in could not be confirmed. It was not retried, so a second key was not created. Start sign-in again.",
            ));
        }
        (
            session.started.clone(),
            session.verifier.clone(),
            Arc::clone(&session.cancel),
        )
    };
    let transport = production_transport()?;
    let sleep_cancel = Arc::clone(&cancel);
    let check_cancel = Arc::clone(&cancel);
    let hooks = SignInHooks {
        now: Box::new(Utc::now),
        sleep: Box::new(move |delay| {
            let slice = Duration::from_millis(200);
            let mut left = delay;
            while !left.is_zero() {
                if sleep_cancel.load(Ordering::Relaxed) {
                    return;
                }
                let step = slice.min(left);
                std::thread::sleep(step);
                left = left.saturating_sub(step);
            }
        }),
        cancelled: Box::new(move || check_cancel.load(Ordering::Relaxed)),
        poll_interval: POLL_INTERVAL,
    };
    {
        let mut guard = lock_slot();
        if let Some(session) = guard.as_mut() {
            if session.generation == generation {
                session.exchange_started = true;
            }
        }
    }
    let result = finish_attempt(&transport, &hooks, &started, &verifier);
    let mut guard = lock_slot();
    let Some(session) = guard.as_mut() else {
        return Err(SignInError::new(
            SignInCode::Cancelled,
            "Browser sign-in was cancelled.",
        ));
    };
    if session.generation != generation {
        return Err(SignInError::new(
            SignInCode::Cancelled,
            "Browser sign-in was cancelled.",
        ));
    }
    match result {
        Ok(key) => {
            session.api_key = Some(key);
            session.verifier.clear();
            Ok(())
        }
        Err(error) => {
            session.verifier.clear();
            if error.code != SignInCode::ExchangeUncertain && error.code != SignInCode::ExchangeFailed
            {
                session.exchange_started = false;
            }
            Err(error)
        }
    }
}

pub fn take_api_key(generation: u64) -> Option<String> {
    let mut guard = lock_slot();
    let session = guard.as_mut()?;
    if session.generation != generation {
        return None;
    }
    session.api_key.take()
}

/// Save an issued key with the existing Mistral Chat Completions profile.
pub fn persist_issued_key(api_key: &str, model_id: Option<&str>) -> Result<String, SignInError> {
    let profile = provider_setup::find_setup_provider(MISTRAL_AUTH_PROVIDER_ID).ok_or_else(|| {
        SignInError::new(
            SignInCode::StartFailed,
            "Mistral provider profile is unavailable.",
        )
    })?;
    let model_id = model_id.unwrap_or(profile.default_model).to_owned();
    provider_setup::persist_provider_setup(&ProviderSetup {
        base_url: profile.base_url.to_owned(),
        api_key: api_key.to_owned(),
        model_id: model_id.clone(),
        model_name: model_id.clone(),
        format: SetupWireFormat::Openai,
        provider_id: MISTRAL_AUTH_PROVIDER_ID.to_owned(),
    })
    .map_err(|_| {
        SignInError::new(
            SignInCode::StartFailed,
            "Could not save the Mistral provider settings.",
        )
    })?;
    Ok(model_id)
}

pub const HEADLESS_GENERATION: u64 = u64::MAX;

/// Open a validated console URL with the platform browser. Failure leaves the
/// caller to show the URL itself.
pub fn try_open_browser(url: &str) -> bool {
    if validate_url(url, BROWSER_BASE_URL).is_err() {
        return false;
    }
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "xdg-open"
    };
    let mut command = std::process::Command::new(program);
    if cfg!(target_os = "windows") {
        command.args(["/C", "start", "", url]);
    } else {
        command.arg(url);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Headless entry used by `/setup mistral browser`. Prints nothing.
pub fn run_production_sign_in(cancel: &Arc<AtomicBool>) -> Result<String, SignInError> {
    authenticate_production(cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Script {
        calls: Mutex<Vec<(String, String, Option<String>)>>,
        handler: Mutex<Vec<Box<dyn Fn(&str, &str, Option<&str>) -> Result<HttpReply, TransportError> + Send>>>,
    }

    impl Script {
        fn new(
            steps: Vec<Box<dyn Fn(&str, &str, Option<&str>) -> Result<HttpReply, TransportError> + Send>>,
        ) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                handler: Mutex::new(steps),
            }
        }
    }

    impl SignInTransport for Script {
        fn call(
            &self,
            method: &str,
            url: &str,
            json_body: Option<String>,
        ) -> Result<HttpReply, TransportError> {
            self.calls.lock().unwrap().push((
                method.to_owned(),
                url.to_owned(),
                json_body.clone(),
            ));
            let mut steps = self.handler.lock().unwrap();
            if steps.is_empty() {
                return Err(TransportError::Network);
            }
            let step = steps.remove(0);
            step(method, url, json_body.as_deref())
        }
    }

    fn ok_json(status: u16, body: serde_json::Value) -> Result<HttpReply, TransportError> {
        Ok(HttpReply {
            status,
            body: body.to_string(),
        })
    }

    fn happy_start() -> serde_json::Value {
        serde_json::json!({
            "process_id": "process-1",
            "sign_in_url": "https://console.mistral.ai/codestral/cli/authenticate#process_id=process-1",
            "poll_url": "https://console.mistral.ai/api/vibe/sign-in/poll/poll-token-1",
            "expires_at": "2026-11-06T12:10:00Z",
        })
    }

    #[test]
    fn pkce_s256_matches_rfc7636_appendix_b() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            code_challenge_s256(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn mock_start_poll_exchange_and_persist_keeps_other_config() {
        let script = Script::new(vec![
            Box::new(|method, url, body| {
                assert_eq!(method, "POST");
                assert_eq!(url, "https://console.mistral.ai/api/vibe/sign-in");
                let body = body.unwrap();
                assert!(body.contains("\"code_challenge_method\":\"S256\""));
                assert!(body.contains("code_challenge"));
                ok_json(200, happy_start())
            }),
            Box::new(|_, url, _| {
                assert_eq!(
                    url,
                    "https://console.mistral.ai/api/vibe/sign-in/poll/poll-token-1"
                );
                ok_json(200, serde_json::json!({"status": "pending"}))
            }),
            Box::new(|_, _, _| {
                ok_json(
                    200,
                    serde_json::json!({"status": "completed", "exchange_token": "exchange-1"}),
                )
            }),
            Box::new(|method, url, body| {
                assert_eq!(method, "POST");
                assert_eq!(
                    url,
                    "https://console.mistral.ai/api/vibe/sign-in/process-1/exchange"
                );
                let body = body.unwrap();
                assert!(body.contains("\"exchange_token\":\"exchange-1\""));
                assert!(body.contains("code_verifier"));
                ok_json(200, serde_json::json!({"api_key": "sk-browser-issued"}))
            }),
        ]);
        let (clock, mut hooks) = SignInHooks::virtual_clock(
            DateTime::parse_from_rfc3339("2026-11-06T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        let _ = clock;
        hooks.poll_interval = Duration::from_secs(1);
        let key = authenticate(&script, &hooks).unwrap();
        assert_eq!(key, "sk-browser-issued");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[ui]\ntheme = \"dark\"\n\n[model_providers.openai]\napi_key = \"sk-other\"\n",
        )
        .unwrap();
        provider_setup::persist_provider_setup_at(
            &path,
            &ProviderSetup {
                base_url: INFERENCE_BASE_URL.to_owned(),
                api_key: key,
                model_id: "large-model-latest".into(),
                model_name: "large-model-latest".into(),
                format: SetupWireFormat::Openai,
                provider_id: MISTRAL_AUTH_PROVIDER_ID.into(),
            },
        )
        .unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("theme = \"dark\""));
        assert!(body.contains("sk-other"));
        assert!(body.contains("sk-browser-issued"));
        assert!(body.contains("https://api.mistral.ai/v1"));
        assert!(body.contains("id = \"mistral-auth\""));
        assert!(body.contains("[provider_credentials.mistral-auth]"));
        assert!(!body.contains("MISTRAL_API_KEY"));
        assert!(!body.contains("exchange-1"));
        assert!(!body.contains("code_verifier"));
    }

    #[test]
    fn pasted_api_key_setup_still_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        provider_setup::persist_provider_setup_at(
            &path,
            &ProviderSetup {
                base_url: INFERENCE_BASE_URL.into(),
                api_key: "pasted-mistral-key".into(),
                model_id: "codestral-latest".into(),
                model_name: "Codestral".into(),
                format: SetupWireFormat::Openai,
                provider_id: "mistral".into(),
            },
        )
        .unwrap();
        let parsed: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            provider_setup::provider_setup_api_key_from_toml(&parsed).as_deref(),
            Some("pasted-mistral-key")
        );
        assert_eq!(
            provider_setup::isolated_provider_api_key(&parsed, "mistral").as_deref(),
            Some("pasted-mistral-key")
        );
        provider_setup::persist_provider_setup_at(
            &path,
            &ProviderSetup {
                base_url: INFERENCE_BASE_URL.into(),
                api_key: "browser-issued-key".into(),
                model_id: "large-model-latest".into(),
                model_name: "Large".into(),
                format: SetupWireFormat::Openai,
                provider_id: MISTRAL_AUTH_PROVIDER_ID.into(),
            },
        )
        .unwrap();
        let parsed: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            provider_setup::isolated_provider_api_key(&parsed, "mistral").as_deref(),
            Some("pasted-mistral-key")
        );
        assert_eq!(
            provider_setup::isolated_provider_api_key(&parsed, "mistral-auth").as_deref(),
            Some("browser-issued-key")
        );
        assert_eq!(
            provider_setup::active_setup_provider_id(&parsed).as_deref(),
            Some("mistral-auth")
        );
        assert_ne!(
            provider_setup::isolated_provider_api_key(&parsed, "mistral").as_deref(),
            provider_setup::isolated_provider_api_key(&parsed, "mistral-auth").as_deref()
        );
    }

    fn during_sign_in() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-06T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn script_until_exchange(
        exchange: Box<dyn Fn(&str, &str, Option<&str>) -> Result<HttpReply, TransportError> + Send>,
    ) -> Script {
        Script::new(vec![
            Box::new(|_, _, _| ok_json(200, happy_start())),
            Box::new(|_, _, _| {
                ok_json(
                    200,
                    serde_json::json!({"status": "completed", "exchange_token": "exchange-secret"}),
                )
            }),
            exchange,
        ])
    }

    #[test]
    fn uncertain_exchange_is_not_retried() {
        let script = script_until_exchange(Box::new(|_, _, _| Err(TransportError::Network)));
        let (_clock, hooks) = SignInHooks::virtual_clock(during_sign_in());
        let error = authenticate(&script, &hooks).unwrap_err();
        assert_eq!(error.code, SignInCode::ExchangeUncertain);
        let posts = script
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, url, _)| method == "POST" && url.contains("/exchange"))
            .count();
        assert_eq!(posts, 1);
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains("exchange-secret"));
        assert!(!rendered.contains("code_verifier"));
    }

    #[test]
    fn exchange_rate_limit_retries_then_accepts_the_key() {
        let refusals = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let seen = std::sync::Arc::clone(&refusals);
        let calls = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let recorded = std::sync::Arc::clone(&calls);
        struct RetryExchange {
            refusals: std::sync::Arc<std::sync::atomic::AtomicU32>,
            calls: std::sync::Arc<Mutex<Vec<String>>>,
        }
        impl SignInTransport for RetryExchange {
            fn call(
                &self,
                method: &str,
                url: &str,
                json_body: Option<String>,
            ) -> Result<HttpReply, TransportError> {
                self.calls.lock().unwrap().push(url.to_owned());
                if method == "POST" && url.ends_with("/vibe/sign-in") {
                    return ok_json(200, happy_start());
                }
                if method == "GET" {
                    return ok_json(
                        200,
                        serde_json::json!({"status": "completed", "exchange_token": "exchange-secret"}),
                    );
                }
                let n = self.refusals.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if n < 2 {
                    ok_json(429, serde_json::json!({"detail": "slow"}))
                } else {
                    let _ = json_body;
                    ok_json(200, serde_json::json!({"api_key": "issued-after-wait"}))
                }
            }
        }
        let transport = RetryExchange {
            refusals: seen,
            calls: recorded,
        };
        let (_clock, hooks) = SignInHooks::virtual_clock(during_sign_in());
        assert_eq!(authenticate(&transport, &hooks).unwrap(), "issued-after-wait");
        let posts = calls
            .lock()
            .unwrap()
            .iter()
            .filter(|url| url.contains("/exchange"))
            .count();
        assert_eq!(posts, 3);
    }

    #[test]
    fn expired_denied_missing_key_rate_limit_and_malformed() {
        let cases: Vec<(
            Box<dyn Fn(&str, &str, Option<&str>) -> Result<HttpReply, TransportError> + Send>,
            SignInCode,
        )> = vec![
            (
                Box::new(|_, _, _| ok_json(200, serde_json::json!({"status": "expired"}))),
                SignInCode::Expired,
            ),
            (
                Box::new(|_, _, _| ok_json(200, serde_json::json!({"status": "denied"}))),
                SignInCode::Denied,
            ),
            (
                Box::new(|_, _, _| ok_json(410, serde_json::json!({}))),
                SignInCode::Expired,
            ),
        ];
        for (step, code) in cases {
            let script = Script::new(vec![
                Box::new(|_, _, _| ok_json(200, happy_start())),
                step,
            ]);
            let (_c, hooks) = SignInHooks::virtual_clock(
                DateTime::parse_from_rfc3339("2026-11-06T12:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            );
            let error = authenticate(&script, &hooks).unwrap_err();
            assert_eq!(error.code, code);
            assert!(!error.to_string().contains("slow-sk-secret"));
            assert!(!format!("{error:?}").contains("slow"));
        }

        let script = Script::new(vec![
            Box::new(|_, _, _| ok_json(200, happy_start())),
            Box::new(|_, _, _| ok_json(429, serde_json::json!({"detail": "slow-sk-secret"}))),
            Box::new(|_, _, _| ok_json(429, serde_json::json!({"detail": "slow-sk-secret"}))),
            Box::new(|_, _, _| ok_json(429, serde_json::json!({"detail": "slow-sk-secret"}))),
        ]);
        let (_c, hooks) = SignInHooks::virtual_clock(
            DateTime::parse_from_rfc3339("2026-11-06T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        let error = authenticate(&script, &hooks).unwrap_err();
        assert_eq!(error.code, SignInCode::RateLimited);
        assert!(!error.to_string().contains("slow-sk-secret"));

        let script = script_until_exchange(Box::new(|_, _, _| {
            ok_json(200, serde_json::json!({"detail": "no key"}))
        }));
        let (_c, hooks) = SignInHooks::virtual_clock(during_sign_in());
        let error = authenticate(&script, &hooks).unwrap_err();
        assert_eq!(error.code, SignInCode::MissingApiKey);

        let script = Script::new(vec![Box::new(|_, _, _| {
            Ok(HttpReply {
                status: 200,
                body: "<html>".into(),
            })
        })]);
        let error = start_process(&script, "challenge").unwrap_err();
        assert_eq!(error.code, SignInCode::StartFailed);

        let script = Script::new(vec![Box::new(|_, _, _| ok_json(429, serde_json::json!({})))]);
        let error = start_process(&script, "challenge").unwrap_err();
        assert_eq!(error.code, SignInCode::RateLimited);
        assert!(!format!("{error:?}").contains("challenge"));
    }

    #[test]
    fn timeout_and_cancel_stop_without_exchange() {
        let script = Script::new(vec![
            Box::new(|_, _, _| ok_json(200, happy_start())),
            Box::new(|_, _, _| ok_json(200, serde_json::json!({"status": "pending"}))),
            Box::new(|_, _, _| ok_json(200, serde_json::json!({"status": "pending"}))),
            Box::new(|_, _, _| ok_json(200, serde_json::json!({"status": "pending"}))),
        ]);
        let (_c, mut hooks) = SignInHooks::virtual_clock(
            DateTime::parse_from_rfc3339("2026-11-06T12:09:59Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        hooks.poll_interval = Duration::from_secs(3);
        let error = authenticate(&script, &hooks).unwrap_err();
        assert_eq!(error.code, SignInCode::TimedOut);
        assert!(
            script
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|(_, url, _)| !url.contains("/exchange"))
        );

        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        let script = Script::new(vec![
            Box::new(move |_, _, _| {
                flag.store(true, Ordering::Relaxed);
                ok_json(200, happy_start())
            }),
            Box::new(|_, _, _| ok_json(200, serde_json::json!({"status": "pending"}))),
        ]);
        let (_c, mut hooks) = SignInHooks::virtual_clock(
            DateTime::parse_from_rfc3339("2026-11-06T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        let flag = Arc::clone(&cancelled);
        hooks.cancelled = Box::new(move || flag.load(Ordering::Relaxed));
        let error = authenticate(&script, &hooks).unwrap_err();
        assert_eq!(error.code, SignInCode::Cancelled);
    }

    #[test]
    fn untrusted_sign_in_and_poll_urls_are_rejected() {
        let evil = serde_json::json!({
            "process_id": "process-1",
            "sign_in_url": "https://evil.example/codestral/cli/authenticate",
            "poll_url": "https://console.mistral.ai/api/vibe/sign-in/poll/t",
            "expires_at": "2026-11-06T12:10:00Z",
        });
        let script = Script::new(vec![Box::new(move |_, _, _| ok_json(200, evil.clone()))]);
        let error = start_process(&script, "c").unwrap_err();
        assert_eq!(error.code, SignInCode::UntrustedUrl);
        assert!(script.calls.lock().unwrap().len() == 1);

        let evil_poll = serde_json::json!({
            "process_id": "process-1",
            "sign_in_url": "https://console.mistral.ai/codestral/cli/authenticate",
            "poll_url": "https://evil.example/api/vibe/sign-in/poll/secret-token",
            "expires_at": "2026-11-06T12:10:00Z",
        });
        let script = Script::new(vec![Box::new(move |_, _, _| ok_json(200, evil_poll.clone()))]);
        let error = start_process(&script, "c").unwrap_err();
        assert_eq!(error.code, SignInCode::UntrustedUrl);
        assert!(!error.to_string().contains("secret-token"));

        let sneaky = serde_json::json!({
            "process_id": "../evil",
            "sign_in_url": "https://console.mistral.ai/authenticate",
            "poll_url": "https://console.mistral.ai/api/vibe/sign-in/poll/t",
            "expires_at": "2026-11-06T12:10:00Z",
        });
        let script = Script::new(vec![Box::new(move |_, _, _| ok_json(200, sneaky.clone()))]);
        assert_eq!(start_process(&script, "c").unwrap_err().code, SignInCode::StartFailed);

        assert!(validate_url("http://console.mistral.ai/api/vibe/sign-in/poll/t", AUTH_API_BASE_URL).is_err());
        assert!(
            validate_url(
                "https://console.mistral.ai:443/api/vibe/sign-in/poll/t",
                AUTH_API_BASE_URL
            )
            .is_ok()
        );
    }

    #[test]
    fn three_poll_network_failures_stop() {
        let script = Script::new(vec![
            Box::new(|_, _, _| ok_json(200, happy_start())),
            Box::new(|_, _, _| Err(TransportError::Network)),
            Box::new(|_, _, _| Err(TransportError::Timeout)),
            Box::new(|_, _, _| Err(TransportError::Network)),
        ]);
        let (_c, hooks) = SignInHooks::virtual_clock(
            DateTime::parse_from_rfc3339("2026-11-06T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        let error = authenticate(&script, &hooks).unwrap_err();
        assert_eq!(error.code, SignInCode::PollFailed);
        assert!(error.to_string().contains("timed out") || error.to_string().contains("could not"));
    }

    #[test]
    fn completed_without_token_does_not_exchange() {
        let script = Script::new(vec![
            Box::new(|_, _, _| ok_json(200, happy_start())),
            Box::new(|_, _, _| ok_json(200, serde_json::json!({"status": "completed"}))),
        ]);
        let (_c, hooks) = SignInHooks::virtual_clock(
            DateTime::parse_from_rfc3339("2026-11-06T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        let error = authenticate(&script, &hooks).unwrap_err();
        assert_eq!(error.code, SignInCode::MissingExchangeToken);
        assert_eq!(script.calls.lock().unwrap().len(), 2);
    }

    #[test]
    fn pinned_join_keeps_api_prefix() {
        assert_eq!(
            pinned_join("https://console.mistral.ai/api", "vibe/sign-in").unwrap(),
            "https://console.mistral.ai/api/vibe/sign-in"
        );
        assert_eq!(
            pinned_join(
                "https://console.mistral.ai/api/",
                "vibe/sign-in/process-1/exchange"
            )
            .unwrap(),
            "https://console.mistral.ai/api/vibe/sign-in/process-1/exchange"
        );
        assert_eq!(
            pinned_join("https://api.mistral.ai/v1", "billing/budget").unwrap(),
            "https://api.mistral.ai/v1/billing/budget"
        );
        let dropped = url::Url::parse("https://console.mistral.ai/api")
            .unwrap()
            .join("vibe/sign-in")
            .unwrap();
        assert_ne!(dropped.path(), "/api/vibe/sign-in");
    }

    #[test]
    fn workspace_9004_and_userinfo_are_rejected_without_secrets() {
        let script = script_until_exchange(Box::new(|_, _, _| {
            ok_json(403, serde_json::json!({"error_code": 9004, "api_key": "sk-leaked"}))
        }));
        let (_c, hooks) = SignInHooks::virtual_clock(during_sign_in());
        let error = authenticate(&script, &hooks).unwrap_err();
        assert_eq!(error.code, SignInCode::ExchangeFailed);
        let rendered = format!("{error} {error:?}");
        assert!(rendered.contains("another workspace"));
        assert!(!rendered.contains("sk-leaked"));

        assert!(
            validate_url(
                "https://user:secret@console.mistral.ai/api/vibe/sign-in/poll/t",
                AUTH_API_BASE_URL
            )
            .is_err()
        );
        assert!(
            validate_url(
                "https://console.mistral.ai/api/../admin/secret",
                AUTH_API_BASE_URL
            )
            .is_err()
        );
    }

    #[test]
    fn successful_poll_resets_the_failure_budget() {
        let script = Script::new(vec![
            Box::new(|_, _, _| ok_json(200, happy_start())),
            Box::new(|_, _, _| Err(TransportError::Network)),
            Box::new(|_, _, _| Err(TransportError::Timeout)),
            Box::new(|_, _, _| ok_json(200, serde_json::json!({"status": "pending"}))),
            Box::new(|_, _, _| Err(TransportError::Network)),
            Box::new(|_, _, _| Err(TransportError::Network)),
            Box::new(|_, _, _| {
                ok_json(
                    200,
                    serde_json::json!({"status": "completed", "exchange_token": "tok"}),
                )
            }),
            Box::new(|_, _, body| {
                assert!(body.unwrap().contains("\"exchange_token\":\"tok\""));
                ok_json(200, serde_json::json!({"api_key": "issued"}))
            }),
        ]);
        let (_c, hooks) = SignInHooks::virtual_clock(
            DateTime::parse_from_rfc3339("2026-11-06T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        assert_eq!(authenticate(&script, &hooks).unwrap(), "issued");
    }

    #[test]
    fn unknown_status_stops() {
        let script = Script::new(vec![
            Box::new(|_, _, _| ok_json(200, happy_start())),
            Box::new(|_, _, _| ok_json(200, serde_json::json!({"status": "weird", "exchange_token": "nope"}))),
        ]);
        let (_c, hooks) = SignInHooks::virtual_clock(
            DateTime::parse_from_rfc3339("2026-11-06T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        let error = authenticate(&script, &hooks).unwrap_err();
        assert_eq!(error.code, SignInCode::UnknownState);
        assert!(!error.to_string().contains("nope"));
    }
}
