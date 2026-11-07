//! Identity headers for documented `opencode.ai` inference endpoints.
//!
//! Supercharge identifies itself. These headers are a documented compatibility
//! signal for routing, not an entitlement claim.

use reqwest::Url;
use uuid::Uuid;

pub const HOST: &str = "opencode.ai";
pub const GO_PROVIDER_ID: &str = "opencode-go";
pub const ZEN_PROVIDER_ID: &str = "opencode-zen";
pub const GO_CHAT_BASE_URL: &str = "https://opencode.ai/inference/go/openai/v1";
pub const ZEN_CHAT_BASE_URL: &str = "https://opencode.ai/inference/openai/v1";
pub const GO_MODELS_URL: &str = "https://opencode.ai/inference/go/v1/models";
pub const ZEN_MODELS_URL: &str = "https://opencode.ai/inference/v1/models";
pub const SESSION_HEADER: &str = "x-opencode-session";

pub fn user_agent() -> String {
    format!("supercharge/{}", supercharge_version::VERSION)
}

pub fn new_session_id() -> String {
    format!("supercharge-{}", Uuid::new_v4().as_hyphenated())
}

/// True only for https://opencode.ai[:443]/inference[...].
pub fn is_canonical_inference_url(url: &Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    if url.host_str() != Some(HOST) {
        return false;
    }
    if url.port_or_known_default() != Some(443) {
        return false;
    }
    let path = url.path();
    path == "/inference" || path.starts_with("/inference/")
}

pub fn is_canonical_inference_url_str(raw: &str) -> bool {
    Url::parse(raw)
        .ok()
        .is_some_and(|url| is_canonical_inference_url(&url))
}

pub fn models_list_url_for(provider_id: Option<&str>, base_url: &str) -> Option<&'static str> {
    if !is_canonical_inference_url_str(base_url) {
        return None;
    }
    match provider_id {
        Some(GO_PROVIDER_ID) => Some(GO_MODELS_URL),
        Some(ZEN_PROVIDER_ID) => Some(ZEN_MODELS_URL),
        _ => None,
    }
}

pub fn identity_headers(session_id: &str) -> Vec<(&'static str, String)> {
    vec![
        ("user-agent", user_agent()),
        (SESSION_HEADER, session_id.to_owned()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_https_inference_paths_match() {
        for raw in [
            GO_CHAT_BASE_URL,
            ZEN_CHAT_BASE_URL,
            GO_MODELS_URL,
            ZEN_MODELS_URL,
            "https://opencode.ai/inference/openai/v1/chat/completions",
            "https://opencode.ai/inference/go/openai/v1/chat/completions",
        ] {
            assert!(is_canonical_inference_url_str(raw), "{raw}");
        }
    }

    #[test]
    fn lookalike_and_unrelated_hosts_do_not_match() {
        for raw in [
            "https://opencode.ai.example/inference/openai/v1",
            "https://not-opencode.ai/inference/openai/v1",
            "http://opencode.ai/inference/openai/v1",
            "https://opencode.ai:8443/inference/openai/v1",
            "https://api.zenllm.org/v1",
            "https://agentrouter.org/v1",
            "https://example.com/inference/openai/v1",
            "https://opencode.ai.attacker.com/inference/openai/v1",
        ] {
            assert!(!is_canonical_inference_url_str(raw), "{raw}");
        }
    }

    #[test]
    fn discovery_urls_are_not_openai_v1_models() {
        assert_eq!(
            models_list_url_for(Some(GO_PROVIDER_ID), GO_CHAT_BASE_URL),
            Some(GO_MODELS_URL)
        );
        assert_eq!(
            models_list_url_for(Some(ZEN_PROVIDER_ID), ZEN_CHAT_BASE_URL),
            Some(ZEN_MODELS_URL)
        );
        assert_ne!(
            GO_MODELS_URL,
            "https://opencode.ai/inference/go/openai/v1/models"
        );
        assert_ne!(
            ZEN_MODELS_URL,
            "https://opencode.ai/inference/openai/v1/models"
        );
        assert_eq!(
            models_list_url_for(Some(GO_PROVIDER_ID), "http://127.0.0.1:9/v1"),
            None
        );
    }

    #[test]
    fn identity_is_supercharge_not_another_product() {
        let headers = identity_headers("supercharge-session-a");
        let ua = headers
            .iter()
            .find(|(n, _)| *n == "user-agent")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(ua, user_agent());
        assert!(ua.starts_with("supercharge/"));
        assert!(!ua.to_ascii_lowercase().contains("opencode/"));
        assert_eq!(
            headers
                .iter()
                .find(|(n, _)| *n == SESSION_HEADER)
                .map(|(_, v)| v.as_str()),
            Some("supercharge-session-a")
        );
        assert!(!headers.iter().any(|(n, _)| n.contains("client")
            || n.contains("project")
            || *n == "x-opencode-client"
            || *n == "x-opencode-project"));
    }
}
