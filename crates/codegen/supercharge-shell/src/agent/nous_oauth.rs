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
