//! Nous Portal device-code OAuth (RFC 8628).
//!
//! Free-tier inference on `inference-api.nousresearch.com` is issued as a
//! Portal JWT, not a static API key. Supercharge stores tokens under
//! `~/.supercharge/nous_oauth.json` and uses them as Bearer credentials.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::util::config::user_config_path;

const PORTAL_BASE: &str = "https://portal.nousresearch.com";
pub const INFERENCE_BASE: &str = "https://inference-api.nousresearch.com/v1";
const DEFAULT_SCOPE: &str = "inference:invoke";
/// Public Portal device-code client id (documented in Nous's open-source CLI).
const DEFAULT_CLIENT_ID: &str = "hermes-cli";
const USER_AGENT: &str = "supercharge-cli";
const REFRESH_SKEW_SECS: u64 = 120;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NousOauthState {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub token_type: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub portal_base_url: String,
    #[serde(default)]
    pub inference_base_url: String,
    #[serde(default)]
    pub expires_at: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct DeviceCodePending {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

fn client_id() -> String {
    std::env::var("SUPERCHARGE_NOUS_OAUTH_CLIENT_ID")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_CLIENT_ID.to_owned())
}

fn oauth_path() -> std::path::PathBuf {
    user_config_path()
        .parent()
        .map(|p| p.join("nous_oauth.json"))
        .unwrap_or_else(|| std::path::PathBuf::from("nous_oauth.json"))
}

fn http_client() -> Result<reqwest::blocking::Client, String> {
    supercharge_extra_ca::build_blocking_reqwest_client(|builder| {
        builder
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(20))
    })
    .map_err(|e| e.to_string())
}

pub fn request_device_code() -> Result<DeviceCodePending, String> {
    let client = http_client()?;
    let cid = client_id();
    let resp = client
        .post(format!("{PORTAL_BASE}/api/oauth/device/code"))
        .header("Accept", "application/json")
        .form(&[("client_id", cid.as_str()), ("scope", DEFAULT_SCOPE)])
        .send()
        .map_err(|e| format!("Nous device-code request failed: {e}"))?;
    let status = resp.status();
    let body = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "Nous device-code HTTP {status}: {}",
            snippet(&body)
        ));
    }
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("Nous device-code JSON: {e}"))?;
    let get = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(str::to_owned)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("Nous device-code missing {k}"))
    };
    Ok(DeviceCodePending {
        device_code: get("device_code")?,
        user_code: get("user_code")?,
        verification_uri: get("verification_uri")
            .unwrap_or_else(|_| format!("{PORTAL_BASE}/oauth/device")),
        verification_uri_complete: v
            .get("verification_uri_complete")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_owned(),
        expires_in: v.get("expires_in").and_then(|x| x.as_u64()).unwrap_or(900),
        interval: v
            .get("interval")
            .and_then(|x| x.as_u64())
            .unwrap_or(5)
            .max(1),
    })
}

/// Poll until the user approves, the code expires, or `authorization_pending` times out.
pub fn poll_for_token(pending: &DeviceCodePending) -> Result<NousOauthState, String> {
    let client = http_client()?;
    let cid = client_id();
    let deadline = SystemTime::now() + Duration::from_secs(pending.expires_in.max(30));
    let mut interval = Duration::from_secs(pending.interval.clamp(1, 5));
    loop {
        if SystemTime::now() >= deadline {
            return Err("Timed out waiting for Nous Portal approval".into());
        }
        let resp = client
            .post(format!("{PORTAL_BASE}/api/oauth/token"))
            .header("Accept", "application/json")
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", cid.as_str()),
                ("device_code", pending.device_code.as_str()),
            ])
            .send()
            .map_err(|e| format!("Nous token poll failed: {e}"))?;
        let status = resp.status();
        let body = resp.text().map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
        if status.is_success() {
            return state_from_token_json(&v, &cid);
        }
        let err = v.get("error").and_then(|x| x.as_str()).unwrap_or("");
        match err {
            "authorization_pending" | "slow_down" => {
                if err == "slow_down" {
                    interval += Duration::from_secs(1);
                }
                std::thread::sleep(interval);
            }
            "expired_token" | "access_denied" => {
                return Err(format!("Nous login {err}"));
            }
            _ => {
                return Err(format!("Nous token HTTP {status}: {}", snippet(&body)));
            }
        }
    }
}

/// One device-code poll step (no sleeping), for clients that drive the poll loop themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePollStatus {
    /// The user has not approved yet; poll again after `interval` seconds.
    Pending,
    /// Poll more slowly (the interval grows by 5 s per RFC 8628).
    SlowDown,
    /// Approved; the token is saved to `nous_oauth.json`.
    Done,
}

/// Exchange the device code once. Saves the token on approval.
pub fn poll_once(pending: &DeviceCodePending) -> Result<DevicePollStatus, String> {
    let client = http_client()?;
    let cid = client_id();
    let resp = client
        .post(format!("{PORTAL_BASE}/api/oauth/token"))
        .header("Accept", "application/json")
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("client_id", cid.as_str()),
            ("device_code", pending.device_code.as_str()),
        ])
        .send()
        .map_err(|e| format!("Nous token poll failed: {e}"))?;
    let status = resp.status();
    let body = resp.text().map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    if status.is_success() {
        let state = state_from_token_json(&v, &cid)?;
        persist_state(&state)?;
        return Ok(DevicePollStatus::Done);
    }
    match v.get("error").and_then(|x| x.as_str()).unwrap_or("") {
        "authorization_pending" => Ok(DevicePollStatus::Pending),
        "slow_down" => Ok(DevicePollStatus::SlowDown),
        err @ ("expired_token" | "access_denied") => Err(format!("Nous login {err}")),
        _ => Err(format!("Nous token HTTP {status}: {}", snippet(&body))),
    }
}

/// Whether a Nous login is saved (the token may still need a refresh).
pub fn signed_in() -> bool {
    load_state().is_some_and(|s| !s.access_token.trim().is_empty() || !s.refresh_token.trim().is_empty())
}

fn state_from_token_json(v: &serde_json::Value, cid: &str) -> Result<NousOauthState, String> {
    let access = v
        .get("access_token")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Nous token response missing access_token".to_string())?;
    let refresh = v
        .get("refresh_token")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_owned();
    let expires_in = v.get("expires_in").and_then(|x| x.as_u64());
    let expires_at =
        jwt_exp(access).or_else(|| expires_in.map(|secs| now_unix().saturating_add(secs)));
    Ok(NousOauthState {
        access_token: access.to_owned(),
        refresh_token: refresh,
        token_type: v
            .get("token_type")
            .and_then(|x| x.as_str())
            .unwrap_or("Bearer")
            .to_owned(),
        scope: v
            .get("scope")
            .and_then(|x| x.as_str())
            .unwrap_or(DEFAULT_SCOPE)
            .to_owned(),
        client_id: cid.to_owned(),
        portal_base_url: PORTAL_BASE.to_owned(),
        inference_base_url: INFERENCE_BASE.to_owned(),
        expires_at,
    })
}

pub fn persist_state(state: &NousOauthState) -> Result<(), String> {
    let path = oauth_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    crate::util::config::atomic_write_string(&path, &json).map_err(|e| e.to_string())
}

pub fn load_state() -> Option<NousOauthState> {
    let path = oauth_path();
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn token_expiring(state: &NousOauthState) -> bool {
    match state.expires_at {
        Some(exp) => now_unix() + REFRESH_SKEW_SECS >= exp,
        None => jwt_exp(&state.access_token)
            .map(|exp| now_unix() + REFRESH_SKEW_SECS >= exp)
            .unwrap_or(false),
    }
}

pub fn refresh_state(state: &NousOauthState) -> Result<NousOauthState, String> {
    if state.refresh_token.trim().is_empty() {
        return Err("Nous refresh token missing; run /setup → Nous again".into());
    }
    let client = http_client()?;
    let cid = if state.client_id.is_empty() {
        client_id()
    } else {
        state.client_id.clone()
    };
    let portal = if state.portal_base_url.is_empty() {
        PORTAL_BASE.to_owned()
    } else {
        state.portal_base_url.trim_end_matches('/').to_owned()
    };
    let resp = client
        .post(format!("{portal}/api/oauth/token"))
        .header("Accept", "application/json")
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", cid.as_str()),
            ("refresh_token", state.refresh_token.as_str()),
        ])
        .send()
        .map_err(|e| format!("Nous refresh failed: {e}"))?;
    let status = resp.status();
    let body = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        let err = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|x| x.as_str()).map(str::to_owned))
            .unwrap_or_default();
        if err == "invalid_grant" || err == "refresh_token_reused" {
            let _ = std::fs::remove_file(oauth_path());
            return Err("Nous login expired; run /setup → Nous Research again".into());
        }
        return Err(format!("Nous refresh HTTP {status}: {}", snippet(&body)));
    }
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("Nous refresh JSON: {e}"))?;
    let mut next = state_from_token_json(&v, &cid)?;
    if next.refresh_token.is_empty() {
        next.refresh_token = state.refresh_token.clone();
    }
    if next.inference_base_url.is_empty() {
        next.inference_base_url = INFERENCE_BASE.to_owned();
    }
    persist_state(&next)?;
    Ok(next)
}

/// Access token suitable as `Authorization: Bearer` for the inference API.
pub fn resolve_access_token() -> Result<String, String> {
    let mut state = load_state().ok_or_else(|| {
        "Nous Portal is not signed in. /setup → Nous Research to log in with OAuth.".to_string()
    })?;
    if token_expiring(&state) {
        state = refresh_state(&state)?;
    }
    if state.access_token.is_empty() {
        return Err("Nous access token empty; run /setup → Nous Research".into());
    }
    Ok(state.access_token)
}

/// Host of an inference base URL, lowercased (`None` when unparsable).
fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url.trim()).ok()?.host_str().map(str::to_ascii_lowercase)
}

/// Whether `base_url` points at the Nous inference API (default host or the one saved at login).
pub(crate) fn covers_base_url(base_url: &str, state: Option<&NousOauthState>) -> bool {
    let Some(host) = host_of(base_url) else {
        return false;
    };
    host_of(INFERENCE_BASE).as_deref() == Some(host.as_str())
        || state
            .filter(|s| !s.inference_base_url.is_empty())
            .and_then(|s| host_of(&s.inference_base_url))
            .as_deref()
            == Some(host.as_str())
}

/// State read from `nous_oauth.json`, re-read whenever the file's mtime or size changes.
fn cached_state_at(path: &std::path::Path) -> Option<NousOauthState> {
    type Entry = (std::path::PathBuf, Option<SystemTime>, u64, Option<NousOauthState>);
    static CACHE: std::sync::Mutex<Option<Entry>> = std::sync::Mutex::new(None);
    let meta = std::fs::metadata(path).ok();
    let mtime = meta.as_ref().and_then(|m| m.modified().ok());
    let len = meta.as_ref().map_or(0, |m| m.len());
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((p, t, l, state)) = guard.as_ref()
        && p == path
        && *t == mtime
        && *l == len
        && mtime.is_some()
    {
        return state.clone();
    }
    let state = meta
        .is_some()
        .then(|| std::fs::read_to_string(path).ok())
        .flatten()
        .and_then(|raw| serde_json::from_str::<NousOauthState>(&raw).ok());
    *guard = Some((path.to_path_buf(), mtime, len, state.clone()));
    state
}

/// Feeds the current Nous access token to every chat request, so a login saved by `/setup`
/// (or a refresh by another process) takes effect without restarting the agent.
#[derive(Debug, Default)]
pub struct NousCredentialSource {
    path_override: Option<std::path::PathBuf>,
}

impl NousCredentialSource {
    fn path(&self) -> std::path::PathBuf {
        self.path_override.clone().unwrap_or_else(oauth_path)
    }
}

impl supercharge_sampler::dynamic_credentials::DynamicCredentialSource for NousCredentialSource {
    fn bearer_for(&self, base_url: &str) -> Option<String> {
        let state = cached_state_at(&self.path());
        if !covers_base_url(base_url, state.as_ref()) {
            return None;
        }
        state.map(|s| s.access_token).filter(|t| !t.trim().is_empty())
    }

    fn prepare<'a>(
        &'a self,
        base_url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let path = self.path();
            let Some(state) = cached_state_at(&path) else {
                return;
            };
            if !covers_base_url(base_url, Some(&state)) || !token_expiring(&state) {
                return;
            }
            // Refresh uses a blocking client; keep it off the async runtime.
            let _ = tokio::task::spawn_blocking(move || refresh_state(&state)).await;
        })
    }
}

/// Register [`NousCredentialSource`] with the sampler once per process.
pub fn install_credential_source() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        supercharge_sampler::dynamic_credentials::register(std::sync::Arc::new(
            NousCredentialSource::default(),
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
        &base64::engine::general_purpose::URL_SAFE,
        padded.as_bytes(),
    )
    .or_else(|_| {
        base64::Engine::decode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            payload.as_bytes(),
        )
    })
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
    fn credential_source_rereads_file_when_it_changes() {
        use supercharge_sampler::dynamic_credentials::DynamicCredentialSource;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nous_oauth.json");
        let write = |tok: &str| {
            let state = NousOauthState {
                access_token: tok.into(),
                refresh_token: "r".into(),
                token_type: "Bearer".into(),
                scope: String::new(),
                client_id: String::new(),
                portal_base_url: String::new(),
                inference_base_url: INFERENCE_BASE.into(),
                expires_at: Some(now_unix() + 3_600),
            };
            std::fs::write(&path, serde_json::to_string(&state).unwrap()).unwrap();
        };
        let src = NousCredentialSource { path_override: Some(path.clone()) };
        write("first-token");
        assert_eq!(src.bearer_for(INFERENCE_BASE).as_deref(), Some("first-token"));
        assert_eq!(src.bearer_for("https://api.other.test/v1"), None);
        // A new login with a different length changes size (and usually mtime).
        write("second-token-after-setup");
        assert_eq!(src.bearer_for(INFERENCE_BASE).as_deref(), Some("second-token-after-setup"));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(src.bearer_for(INFERENCE_BASE), None);
    }

    #[test]
    fn jwt_exp_reads_payload() {
        // {"exp":1700000000} compact
        let payload = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            br#"{"exp":1700000000}"#,
        );
        let token = format!("hdr.{payload}.sig");
        assert_eq!(jwt_exp(&token), Some(1_700_000_000));
    }
}
