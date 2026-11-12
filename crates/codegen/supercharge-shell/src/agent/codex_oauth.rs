//! Codex CLI browser sign-in (OAuth 2.0 Authorization Code + PKCE), shared with
//! the Codex CLI's own credential store.
//!
//! ChatGPT-account inference is issued as an OAuth token, not an API key. This
//! module mirrors the Codex CLI flow: a localhost callback on 127.0.0.1:1455
//! (fallback 1457, kept in sync with the Codex CLI Hydra allow-list), tokens
//! exchanged at `auth.openai.com/oauth/token`, and state persisted to
//! `~/.codex/auth.json` so both tools can refresh the same login.
//!
//! Verified live (2026-11-07): `POST {chatgpt backend}/responses` needs only
//! `Authorization: Bearer <access_token>` (plus `chatgpt-account-id` for
//! workspace accounts); refresh tokens rotate on every refresh, so the saved
//! file must be rewritten after each refresh.

use std::io::{Read, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use supercharge_sampler::dynamic_credentials::DynamicCredentialSource;

/// Codex CLI's public OAuth client id (codex-rs/login/src/auth/manager.rs).
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const ISSUER: &str = "https://auth.openai.com";
pub const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const CODEX_CALLBACK_PORT: u16 = 1455;
pub const CODEX_CALLBACK_FALLBACK_PORT: u16 = 1457;
/// ChatGPT-account Responses endpoint (codex model-provider-info CHATGPT_CODEX_BASE_URL).
pub const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
pub const CODEX_PROVIDER_ID: &str = "codex";
/// Curated Codex slugs for ChatGPT accounts (gpt-5.x names are rejected).
pub const CURATED_MODELS: &[&str] = &["gpt-6.1-terra", "gpt-6.1-sol", "gpt-6-luna", "gpt-6-astra"];
const USER_AGENT: &str = "supercharge-cli";
const REFRESH_SKEW_SECS: u64 = 300;
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexTokens {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub account_id: String,
}

/// `$CODEX_HOME/auth.json` layout (codex-rs/login/src/auth/storage.rs).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexAuthDotJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<CodexTokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refresh: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexSignInStarted {
    pub sign_in_url: String,
    pub port: u16,
    /// PKCE verifier kept in process memory until the exchange.
    pub verifier: String,
    pub state: String,
}

/// One localhost callback exchange. Holds the TCP listener so the port stays
/// bound from `begin` to `wait`; `auth.json` is written on success.
pub struct CodexSignInSession {
    pub started: CodexSignInStarted,
    listener: std::net::TcpListener,
}

/// Live sign-in for one setup generation, kept process-wide so the UI thread
/// can start it and a background task can wait on it (mistral's pattern).
struct LiveSignIn {
    generation: u64,
    started: CodexSignInStarted,
    listener: Option<std::net::TcpListener>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

fn live_slot() -> &'static std::sync::Mutex<Option<LiveSignIn>> {
    static SLOT: std::sync::OnceLock<std::sync::Mutex<Option<LiveSignIn>>> =
        std::sync::OnceLock::new();
    SLOT.get_or_init(|| std::sync::Mutex::new(None))
}

fn lock_live_slot() -> std::sync::MutexGuard<'static, Option<LiveSignIn>> {
    live_slot().lock().unwrap_or_else(|e| e.into_inner())
}

fn codex_home() -> std::path::PathBuf {
    if let Some(home) = supercharge_dirs::home_dir() {
        home.join(".codex")
    } else {
        std::path::PathBuf::from(".codex")
    }
}

pub fn auth_dot_json_path() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("CODEX_HOME")
        && !home.trim().is_empty()
    {
        return std::path::PathBuf::from(home).join("auth.json");
    }
    codex_home().join("auth.json")
}

pub fn load_auth() -> Option<CodexAuthDotJson> {
    let raw = std::fs::read_to_string(auth_dot_json_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn persist_auth(auth: &CodexAuthDotJson) -> Result<(), String> {
    let path = auth_dot_json_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(auth).map_err(|e| e.to_string())?;
    crate::util::config::atomic_write_string(&path, &json).map_err(|e| e.to_string())
}

/// Whether a ChatGPT-mode login is saved (tokens may still need a refresh).
pub fn signed_in() -> bool {
    load_auth().is_some_and(|a| a.auth_mode.as_deref() == Some("chatgpt") && a.tokens.is_some())
}

fn http_client() -> Result<reqwest::blocking::Client, String> {
    supercharge_extra_ca::build_blocking_reqwest_client(|builder| {
        builder
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
    })
    .map_err(|e| e.to_string())
}

/// RFC 7636 S256 challenge; unpadded base64url of SHA-256(verifier).
pub fn code_challenge_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// PKCE verifier (48 OS CSPRNG bytes, base64url) and its S256 challenge.
pub fn generate_pkce_pair() -> std::io::Result<(String, String)> {
    let verifier = crate::agent::os_random::secure_token_b64url::<48>()?;
    let challenge = code_challenge_s256(&verifier);
    Ok((verifier, challenge))
}

/// OAuth `state` (32 OS CSPRNG bytes, base64url).
fn generate_state() -> std::io::Result<String> {
    crate::agent::os_random::secure_token_b64url::<32>()
}

/// Authorize URL with the same shape as the Codex CLI (scope + originator
/// extra params; simplified-flow flags are CLI-specific and omitted).
pub fn build_authorize_url(redirect_uri: &str, challenge: &str, state: &str) -> String {
    let mut url = url::Url::parse(&format!("{ISSUER}/oauth/authorize")).expect("static issuer");
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", CLIENT_ID);
        query.append_pair("redirect_uri", redirect_uri);
        query.append_pair("scope", "openid profile email offline_access");
        query.append_pair("code_challenge", challenge);
        query.append_pair("code_challenge_method", "S256");
        query.append_pair("state", state);
        query.append_pair("id_token_add_organizations", "true");
        query.append_pair("originator", "codex_cli_rs");
    }
    url.to_string()
}

/// Bind the localhost callback listener, preferring 1455 then 1457.
fn bind_callback_listener() -> std::io::Result<(std::net::TcpListener, u16)> {
    for port in [CODEX_CALLBACK_PORT, CODEX_CALLBACK_FALLBACK_PORT] {
        match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => return Ok((listener, port)),
            Err(_) => continue,
        }
    }
    Err(std::io::Error::other(
        "ports 1455 and 1457 are both in use; free one and retry",
    ))
}

/// Start a browser sign-in for this setup generation: bind the callback
/// listener, build the URL, and keep the verifier in process memory.
/// Returns the started session (URL included) for display; the live slot
/// keeps the listener.
pub fn begin_live(generation: u64) -> Result<CodexSignInStarted, String> {
    let (listener, port) = bind_callback_listener().map_err(|e| e.to_string())?;
    let (verifier, challenge) = generate_pkce_pair()
        .map_err(|e| format!("Could not generate a secure sign-in code: {e}"))?;
    let state =
        generate_state().map_err(|e| format!("Could not generate a secure sign-in code: {e}"))?;
    let redirect_uri = format!("http://127.0.0.1:{port}/auth/callback");
    let sign_in_url = build_authorize_url(&redirect_uri, &challenge, &state);
    let started = CodexSignInStarted {
        sign_in_url: sign_in_url.clone(),
        port,
        verifier,
        state,
    };
    *lock_live_slot() = Some(LiveSignIn {
        generation,
        started: started.clone(),
        listener: Some(listener),
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    Ok(started)
}

/// The authorize URL for a live sign-in (`None` when the generation is stale).
pub fn sign_in_url(generation: u64) -> Option<String> {
    lock_live_slot()
        .as_ref()
        .filter(|session| session.generation == generation)
        .map(|session| session.started.sign_in_url.clone())
}

/// Cancel a live sign-in: the waiter returns an error and the listener drops.
pub fn cancel_live(generation: u64) {
    if let Some(session) = lock_live_slot().as_ref() {
        if session.generation == generation {
            session
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Wait for the browser callback of a live sign-in, exchange the code, and
/// persist tokens to `auth.json`. Blocks the calling thread; run it off the
/// UI thread.
pub fn wait_live(generation: u64) -> Result<(), String> {
    let (started, listener, cancel) = {
        let mut guard = lock_live_slot();
        let Some(session) = guard.as_mut() else {
            return Err("Codex sign-in is not in progress.".into());
        };
        if session.generation != generation {
            return Err("Codex sign-in was cancelled.".into());
        }
        let Some(listener) = session.listener.take() else {
            return Err("Codex sign-in was cancelled.".into());
        };
        (
            session.started.clone(),
            listener,
            std::sync::Arc::clone(&session.cancel),
        )
    };
    let session = CodexSignInSession { started, listener };
    let result = wait_for_callback(&session, &cancel);
    // Clear the slot either way; a finished or failed sign-in has no callback left.
    let mut guard = lock_live_slot();
    if guard
        .as_ref()
        .is_some_and(|live| live.generation == generation)
    {
        *guard = None;
    }
    result.map(|_| ())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Callback {
    Code(String),
    Denied(String),
    StateMismatch,
    Cancelled,
}

fn parse_callback(path_and_query: &str, expected_state: &str) -> Result<Callback, std::io::Error> {
    let url = url::Url::parse(&format!("http://127.0.0.1{path_and_query}"))
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    if url.path() != "/auth/callback" {
        return Ok(Callback::Cancelled);
    }
    let get = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    if get("state").as_deref() != Some(expected_state) {
        return Ok(Callback::StateMismatch);
    }
    if let Some(error) = get("error") {
        let description = get("error_description").unwrap_or_default();
        return Ok(Callback::Denied(if description.is_empty() {
            error
        } else {
            format!("{error}: {description}")
        }));
    }
    match get("code").filter(|code| !code.is_empty()) {
        Some(code) => Ok(Callback::Code(code)),
        None => Ok(Callback::Cancelled),
    }
}

const CALLBACK_SUCCESS_BODY: &str =
    "Sign-in complete. You can close this tab and return to the terminal.";
const CALLBACK_FAILURE_BODY: &str = "Sign-in failed. Check the terminal for details.";

fn http_reply(mut stream: std::net::TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// Read one HTTP request off the callback connection, tolerating slow browsers.
fn read_request_line(stream: &mut std::net::TcpStream) -> std::io::Result<String> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut buffer = [0u8; 4096];
    let read = stream.read(&mut buffer)?;
    let raw = String::from_utf8_lossy(&buffer[..read]);
    let line = raw.lines().next().unwrap_or_default().to_owned();
    if line.is_empty() {
        return Err(std::io::Error::other("empty callback request"));
    }
    Ok(line)
}

/// Wait for the browser callback, then exchange the code for tokens. The
/// saved `auth.json` is updated in place so the Codex CLI stays logged in.
pub fn wait_for_callback(
    session: &CodexSignInSession,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<CodexAuthDotJson, String> {
    use std::sync::atomic::Ordering;
    let deadline = SystemTime::now() + Duration::from_secs(600);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Codex sign-in was cancelled.".into());
        }
        if SystemTime::now() >= deadline {
            return Err("Codex sign-in timed out.".into());
        }
        session
            .listener
            .set_nonblocking(true)
            .map_err(|e| e.to_string())?;
        match session.listener.accept() {
            Ok((mut stream, _)) => {
                let request_line = match read_request_line(&mut stream) {
                    Ok(line) => line,
                    Err(_) => {
                        http_reply(stream, "400 Bad Request", CALLBACK_FAILURE_BODY);
                        continue;
                    }
                };
                let target = request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_owned();
                match parse_callback(&target, &session.started.state) {
                    Ok(Callback::Code(code)) => {
                        http_reply(stream, "200 OK", CALLBACK_SUCCESS_BODY);
                        let auth = exchange_code(&code, &session.started)?;
                        persist_auth(&auth)?;
                        return Ok(auth);
                    }
                    Ok(Callback::Denied(reason)) => {
                        http_reply(stream, "200 OK", CALLBACK_FAILURE_BODY);
                        return Err(format!("Codex sign-in was denied: {reason}"));
                    }
                    Ok(Callback::StateMismatch) => {
                        http_reply(stream, "400 Bad Request", CALLBACK_FAILURE_BODY);
                        return Err("Codex sign-in failed: callback state mismatch.".into());
                    }
                    Ok(Callback::Cancelled) => {
                        http_reply(stream, "404 Not Found", CALLBACK_FAILURE_BODY);
                        continue;
                    }
                    Err(error) => {
                        http_reply(stream, "400 Bad Request", CALLBACK_FAILURE_BODY);
                        return Err(format!("Codex callback error: {error}"));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(error) => return Err(format!("Codex callback listener failed: {error}")),
        }
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

fn token_request(form: &[(&str, &str)]) -> Result<TokenResponse, String> {
    let client = http_client()?;
    let resp = client
        .post(TOKEN_URL)
        .header("Accept", "application/json")
        .form(form)
        .send()
        .map_err(|e| format!("Codex token request failed: {e}"))?;
    let status = resp.status();
    let body = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        let detail = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                let error = v.get("error").and_then(|x| x.as_str()).unwrap_or("");
                let description = v
                    .get("error_description")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                (!error.is_empty()).then(|| {
                    if description.is_empty() {
                        error.to_owned()
                    } else {
                        format!("{error}: {description}")
                    }
                })
            })
            .unwrap_or_else(|| snippet(&body));
        return Err(format!(
            "Codex token endpoint returned HTTP {status}: {detail}"
        ));
    }
    serde_json::from_str(&body).map_err(|e| format!("Codex token response invalid: {e}"))
}

fn account_id_from_id_token(id_token: &str) -> String {
    let payload = id_token.split('.').nth(1).unwrap_or_default();
    let padded = match payload.len() % 4 {
        0 => payload.to_owned(),
        n => format!("{payload}{}", "=".repeat(4 - n)),
    };
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        padded.as_bytes(),
    )
    .unwrap_or_default();
    let claims: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    claims
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_owned()
}

fn auth_from_token_response(
    response: TokenResponse,
    previous: Option<&CodexAuthDotJson>,
) -> CodexAuthDotJson {
    let id_token = response
        .id_token
        .or_else(|| {
            previous
                .and_then(|p| p.tokens.as_ref())
                .map(|t| t.id_token.clone())
        })
        .unwrap_or_default();
    let refresh_token = response
        .refresh_token
        .or_else(|| {
            previous
                .and_then(|p| p.tokens.as_ref())
                .map(|t| t.refresh_token.clone())
        })
        .unwrap_or_default();
    let account_id = {
        let from_jwt = account_id_from_id_token(&id_token);
        if from_jwt.is_empty() {
            previous
                .and_then(|p| p.tokens.as_ref())
                .map(|t| t.account_id.clone())
                .unwrap_or_default()
        } else {
            from_jwt
        }
    };
    CodexAuthDotJson {
        auth_mode: Some("chatgpt".into()),
        openai_api_key: previous.and_then(|p| p.openai_api_key.clone()),
        tokens: Some(CodexTokens {
            id_token,
            access_token: response.access_token,
            refresh_token,
            account_id,
        }),
        last_refresh: Some(rfc3339_now()),
    }
}

fn rfc3339_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Exchange an authorization code for tokens (PKCE, public client).
pub fn exchange_code(code: &str, started: &CodexSignInStarted) -> Result<CodexAuthDotJson, String> {
    let previous = load_auth();
    let redirect_uri = format!("http://127.0.0.1:{}/auth/callback", started.port);
    let response = token_request(&[
        ("grant_type", "authorization_code"),
        ("client_id", CLIENT_ID),
        ("code", code),
        ("redirect_uri", redirect_uri.as_str()),
        ("code_verifier", started.verifier.as_str()),
    ])?;
    let auth = auth_from_token_response(response, previous.as_ref());
    if auth
        .tokens
        .as_ref()
        .is_some_and(|t| t.refresh_token.is_empty())
    {
        return Err("Codex token response missing refresh_token".into());
    }
    Ok(auth)
}

/// Refresh the saved login in place. Returns the refreshed auth.
pub fn refresh_auth() -> Result<CodexAuthDotJson, String> {
    let auth = load_auth().ok_or_else(|| {
        "Codex is not signed in. Run /setup → ChatGPT (Codex) to log in.".to_string()
    })?;
    let tokens = auth
        .tokens
        .as_ref()
        .ok_or_else(|| "Codex login has no tokens; run /setup → ChatGPT (Codex).".to_string())?;
    if tokens.refresh_token.trim().is_empty() {
        return Err("Codex refresh token missing; run /setup → ChatGPT (Codex) again.".into());
    }
    let response = token_request(&[
        ("grant_type", "refresh_token"),
        ("client_id", CLIENT_ID),
        ("refresh_token", tokens.refresh_token.as_str()),
    ])?;
    let next = auth_from_token_response(response, Some(&auth));
    persist_auth(&next)?;
    Ok(next)
}

fn token_expiring(auth: &CodexAuthDotJson) -> bool {
    let Some(tokens) = auth.tokens.as_ref() else {
        return false;
    };
    match jwt_exp(&tokens.access_token) {
        Some(exp) => now_unix() + REFRESH_SKEW_SECS >= exp,
        // Unknown expiry: codex refreshes on an 8-day cadence; treat unknown as fresh.
        None => false,
    }
}

/// Access token suitable as `Authorization: Bearer` for the ChatGPT codex API.
pub fn resolve_access_token() -> Result<String, String> {
    let auth = load_auth().ok_or_else(|| {
        "Codex is not signed in. Run /setup → ChatGPT (Codex) to log in.".to_string()
    })?;
    let tokens = auth
        .tokens
        .as_ref()
        .filter(|t| !t.access_token.trim().is_empty())
        .ok_or_else(|| "Codex login has no tokens; run /setup → ChatGPT (Codex).".to_string())?;
    Ok(tokens.access_token.clone())
}

/// Host of a base URL, lowercased (`None` when unparsable).
fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url.trim())
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}

/// Whether `base_url` points at the ChatGPT codex backend.
pub(crate) fn covers_base_url(base_url: &str) -> bool {
    host_of(base_url).is_some_and(|host| host == "chatgpt.com")
}

/// Feeds the current Codex access token to every request against the ChatGPT
/// codex backend, refreshing it in `auth.json` when it nears expiry.
#[derive(Debug, Default)]
pub struct CodexCredentialSource {
    path_override: Option<std::path::PathBuf>,
}

impl CodexCredentialSource {
    fn path(&self) -> std::path::PathBuf {
        self.path_override
            .clone()
            .unwrap_or_else(auth_dot_json_path)
    }
}

impl DynamicCredentialSource for CodexCredentialSource {
    fn bearer_for(&self, base_url: &str) -> Option<String> {
        if !covers_base_url(base_url) {
            return None;
        }
        let raw = std::fs::read_to_string(self.path()).ok()?;
        let auth: CodexAuthDotJson = serde_json::from_str(&raw).ok()?;
        auth.tokens
            .map(|t| t.access_token)
            .filter(|t| !t.trim().is_empty())
    }

    fn prepare<'a>(
        &'a self,
        base_url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if !covers_base_url(base_url) {
                return;
            }
            let path = self.path();
            let Ok(raw) = std::fs::read_to_string(&path) else {
                return;
            };
            let Ok(auth) = serde_json::from_str::<CodexAuthDotJson>(&raw) else {
                return;
            };
            if !token_expiring(&auth) {
                return;
            }
            // Refresh uses a blocking client; keep it off the async runtime.
            let _ = tokio::task::spawn_blocking(refresh_auth).await;
        })
    }
}

/// Register [`CodexCredentialSource`] with the sampler once per process.
pub fn install_credential_source() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        supercharge_sampler::dynamic_credentials::register(std::sync::Arc::new(
            CodexCredentialSource::default(),
        ));
    });
}

fn jwt_exp(token: &str) -> Option<u64> {
    let payload = token.split('.').nth(1)?;
    let padded = match payload.len() % 4 {
        0 => payload.to_owned(),
        n => format!("{payload}{}", "=".repeat(4 - n)),
    };
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        padded.as_bytes(),
    )
    .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("exp")?.as_u64()
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn snippet(body: &str) -> String {
    body.trim().chars().take(240).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_challenge_matches_rfc_7636_vector() {
        // RFC 7636 appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            code_challenge_s256(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn generated_pkce_pair_is_valid_and_unique() {
        let (verifier, challenge) = generate_pkce_pair().unwrap();
        assert_eq!(verifier.len(), 64);
        assert!((43..=128).contains(&verifier.len()));
        assert!(
            verifier
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
        assert_eq!(challenge, code_challenge_s256(&verifier));
        assert_eq!(challenge.len(), 43);
        let (verifier2, challenge2) = generate_pkce_pair().unwrap();
        assert_ne!(verifier, verifier2);
        assert_ne!(challenge, challenge2);
    }

    #[test]
    fn pkce_generation_does_not_open_a_device_path() {
        let src = include_str!("codex_oauth.rs");
        let needle = ["/dev/", "urandom"].concat();
        assert!(!src.contains(&needle));
    }

    #[test]
    fn generated_state_is_valid_and_unique() {
        let a = generate_state().unwrap();
        assert_eq!(a.len(), 43);
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
        assert_ne!(a, generate_state().unwrap());
    }

    #[test]
    fn authorize_url_carries_pkce_and_state() {
        let url = build_authorize_url(
            "http://127.0.0.1:1455/auth/callback",
            "challenge-x",
            "state-y",
        );
        let parsed = url::Url::parse(&url).unwrap();
        assert_eq!(parsed.host_str(), Some("auth.openai.com"));
        assert_eq!(parsed.path(), "/oauth/authorize");
        let get = |key: &str| {
            parsed
                .query_pairs()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.into_owned())
        };
        assert_eq!(get("response_type").as_deref(), Some("code"));
        assert_eq!(get("client_id").as_deref(), Some(CLIENT_ID));
        assert_eq!(
            get("redirect_uri").as_deref(),
            Some("http://127.0.0.1:1455/auth/callback")
        );
        assert_eq!(get("code_challenge").as_deref(), Some("challenge-x"));
        assert_eq!(get("code_challenge_method").as_deref(), Some("S256"));
        assert_eq!(get("state").as_deref(), Some("state-y"));
        assert_eq!(
            get("scope").as_deref(),
            Some("openid profile email offline_access")
        );
    }

    #[test]
    fn parse_callback_extracts_code_and_validates_state() {
        let ok = parse_callback("/auth/callback?code=abc&state=expected", "expected").unwrap();
        assert_eq!(ok, Callback::Code("abc".into()));

        let mismatch = parse_callback("/auth/callback?code=abc&state=other", "expected").unwrap();
        assert_eq!(mismatch, Callback::StateMismatch);

        let denied = parse_callback(
            "/auth/callback?state=expected&error=access_denied&error_description=nope",
            "expected",
        )
        .unwrap();
        assert_eq!(denied, Callback::Denied("access_denied: nope".into()));

        let other_path = parse_callback("/cancel", "expected").unwrap();
        assert_eq!(other_path, Callback::Cancelled);
    }

    #[test]
    fn account_id_reads_openai_auth_claim() {
        // {"https://api.openai.com/auth":{"chatgpt_account_id":"acct-1"}}
        let payload = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct-1"}}"#,
        );
        let token = format!("hdr.{payload}.sig");
        assert_eq!(account_id_from_id_token(&token), "acct-1");
        assert_eq!(account_id_from_id_token("not-a-jwt"), "");
    }

    #[test]
    fn covers_base_url_only_matches_chatgpt_host() {
        assert!(covers_base_url(CHATGPT_CODEX_BASE_URL));
        assert!(covers_base_url("https://chatgpt.com/backend-api/codex/"));
        assert!(!covers_base_url("https://api.openai.com/v1"));
        assert!(!covers_base_url("https://chatgpt.com.evil.test/codex"));
    }

    #[test]
    fn credential_source_rereads_file_when_it_changes() {
        use supercharge_sampler::dynamic_credentials::DynamicCredentialSource;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let write = |tok: &str, refresh: &str| {
            let auth = CodexAuthDotJson {
                auth_mode: Some("chatgpt".into()),
                openai_api_key: None,
                tokens: Some(CodexTokens {
                    id_token: String::new(),
                    access_token: tok.into(),
                    refresh_token: refresh.into(),
                    account_id: "acct".into(),
                }),
                last_refresh: None,
            };
            std::fs::write(&path, serde_json::to_string(&auth).unwrap()).unwrap();
        };
        let src = CodexCredentialSource {
            path_override: Some(path.clone()),
        };
        write("first-token", "r1");
        assert_eq!(
            src.bearer_for(CHATGPT_CODEX_BASE_URL).as_deref(),
            Some("first-token")
        );
        assert_eq!(src.bearer_for("https://api.openai.com/v1"), None);
        write("second-token", "r2");
        assert_eq!(
            src.bearer_for(CHATGPT_CODEX_BASE_URL).as_deref(),
            Some("second-token")
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(src.bearer_for(CHATGPT_CODEX_BASE_URL), None);
    }

    #[test]
    fn auth_json_round_trips_and_keeps_unknown_fields() {
        let raw = r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"id_token":"i","access_token":"a","refresh_token":"r","account_id":"acct"},"last_refresh":"2026-11-02T06:01:15Z","unknown_future_field":42}"#;
        let auth: CodexAuthDotJson = serde_json::from_str(raw).unwrap();
        assert_eq!(auth.auth_mode.as_deref(), Some("chatgpt"));
        let tokens = auth.tokens.as_ref().unwrap();
        assert_eq!(tokens.access_token, "a");
        assert_eq!(tokens.account_id, "acct");
    }
}
