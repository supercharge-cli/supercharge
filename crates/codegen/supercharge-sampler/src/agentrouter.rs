//! Client identity for the agentrouter.org gateway.
//!
//! The gateway rejects ("unauthorized client detected") any request whose headers
//! do not look like a ZCode client, so requests must carry the fingerprint
//! captured from a live ZCode 3.16.2 run. `ai-proxy` mirrors the same set.
//!
//! The static identity is applied once per client in `SamplingClient::new`; the
//! per-request ids are refreshed on every `post` so long sessions do not reuse
//! one query/request id thousands of times.

use uuid::Uuid;

/// Base URL host that triggers the identity headers.
pub const HOST: &str = "agentrouter.org";

/// `user-agent` ZCode 3.16.2 sends; the gateway checks this one first.
pub const USER_AGENT_VALUE: &str = "ZCode/3.16.2 ai-sdk/provider-utils/4.0.27 runtime/node.js/24";

/// True when `base_url` points at the agentrouter gateway (any scheme or path).
pub fn is_agentrouter_base_url(base_url: &str) -> bool {
    match reqwest::Url::parse(base_url) {
        Ok(url) => url
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case(HOST)),
        // Not a parseable URL (should not happen): fall back to a substring check.
        Err(_) => base_url.to_ascii_lowercase().contains(HOST),
    }
}

/// One identity per client, the way one ZCode process owns one session.
pub fn new_session_id() -> String {
    format!("session_{}", Uuid::new_v4().simple())
}

/// Node's `os.release()` is the kernel release; read it the cheap way.
fn os_version() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| std::env::consts::OS.to_owned())
}

fn node_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

fn node_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

fn os_category() -> &'static str {
    match std::env::consts::OS {
        "macos" => "mac",
        "windows" => "win",
        other => other,
    }
}

/// Stable headers for one client/session, application order matters for
/// `user-agent`: callers must apply these after their own default `User-Agent`.
pub fn static_identity_headers(session_id: &str) -> Vec<(&'static str, String)> {
    vec![
        ("user-agent", USER_AGENT_VALUE.to_owned()),
        ("http-referer", "https://zcode.z.ai".to_owned()),
        ("x-client-language", "en-US".to_owned()),
        ("x-client-timezone", "Asia/Calcutta".to_owned()),
        ("x-os-category", os_category().to_owned()),
        ("x-os-version", os_version()),
        (
            "x-platform",
            format!("{}-{}", node_platform(), node_arch()),
        ),
        ("x-release-channel", "production".to_owned()),
        ("x-title", "Z Code@electron".to_owned()),
        ("x-zcode-agent", "glm".to_owned()),
        ("x-zcode-app-version", "3.16.2".to_owned()),
        ("x-zcode-query-source", "main_turn".to_owned()),
        ("x-zcode-session-type", "main".to_owned()),
        ("x-session-id", session_id.to_owned()),
    ]
}

/// Fresh ids for every request, as ZCode sends them.
pub fn per_request_headers() -> Vec<(&'static str, String)> {
    vec![
        ("x-query-id", Uuid::new_v4().to_string()),
        ("x-request-id", Uuid::new_v4().to_string()),
        ("x-zcode-trace-id", Uuid::new_v4().to_string()),
    ]
}
