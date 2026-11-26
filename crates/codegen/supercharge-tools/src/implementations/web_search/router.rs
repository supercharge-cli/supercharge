//! Routes `web_search` to Exa, then Linkup, then the existing Responses API client.
//!
//! Exa is used when a key is configured. Linkup runs when Exa is unset or the Exa call fails.
//! With neither key, [`WebSearchClient`](super::client::WebSearchClient) is used unchanged.

use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

use super::client::WebSearchClient;
use super::types::WebSearchConfig;

/// Bound on every Exa and Linkup HTTP call.
pub const SEARCH_HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RESULTS: u32 = 10;
const SNIPPET_CHARS: usize = 2_000;
const PAGE_TEXT_CHARS: usize = 8_000;

const EXA_ORIGIN: &str = "https://api.exa.ai";
const LINKUP_ORIGIN: &str = "https://api.linkup.so";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMode {
    Auto,
    Lite,
}

impl SearchMode {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(Self::Auto),
            Some("auto") => Ok(Self::Auto),
            Some("lite") => Ok(Self::Lite),
            Some(other) => Err(format!(
                "web_search mode must be \"auto\" or \"lite\", got \"{other}\""
            )),
        }
    }

    fn exa_type(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Lite => "lite",
        }
    }

    fn linkup_depth(self) -> &'static str {
        match self {
            Self::Auto => "standard",
            Self::Lite => "fast",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum FailureClass {
    Timeout,
    Unauthorized,
    RateLimited,
    PaymentRequired,
    BadRequest,
    Server,
    Network,
    Unparseable,
}

impl FailureClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Unauthorized => "unauthorized",
            Self::RateLimited => "rate limited",
            Self::PaymentRequired => "payment required",
            Self::BadRequest => "bad request",
            Self::Server => "server error",
            Self::Network => "network error",
            Self::Unparseable => "unparseable response",
        }
    }
}

/// Search failure whose Display and Debug never contain API keys.
#[derive(Clone)]
pub struct SearchFailure {
    backend: &'static str,
    class: FailureClass,
    detail: String,
}

impl std::fmt::Display for SearchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.detail.is_empty() {
            write!(
                f,
                "{} search failed ({})",
                self.backend,
                self.class.as_str()
            )
        } else {
            write!(
                f,
                "{} search failed ({}): {}",
                self.backend,
                self.class.as_str(),
                self.detail
            )
        }
    }
}

impl std::fmt::Debug for SearchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchFailure")
            .field("backend", &self.backend)
            .field("class", &self.class.as_str())
            .field("detail", &self.detail)
            .finish()
    }
}

impl SearchFailure {
    fn new(
        backend: &'static str,
        class: FailureClass,
        detail: impl Into<String>,
        keys: &[&str],
    ) -> Self {
        Self {
            backend,
            class,
            detail: scrub(&detail.into(), keys),
        }
    }
}

fn scrub(text: &str, keys: &[&str]) -> String {
    let mut out = text.to_string();
    for key in keys {
        let key = key.trim();
        if key.len() >= 4 {
            out = out.replace(key, "[redacted]");
        }
    }
    if out.len() > 300 {
        out.truncate(300);
    }
    out
}

#[derive(Clone)]
pub struct WebSearchRouter {
    responses: Option<WebSearchClient>,
    exa_key: Option<String>,
    linkup_key: Option<String>,
    default_allowed: Option<Vec<String>>,
    default_excluded: Option<Vec<String>>,
    http: reqwest::Client,
    exa_origin: String,
    linkup_origin: String,
}

impl std::fmt::Debug for WebSearchRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSearchRouter")
            .field("has_responses", &self.responses.is_some())
            .field("has_exa", &self.exa_key.is_some())
            .field("has_linkup", &self.linkup_key.is_some())
            .finish()
    }
}

impl WebSearchRouter {
    pub fn from_config(
        config: &WebSearchConfig,
        responses: Option<WebSearchClient>,
    ) -> Option<Self> {
        if !config.is_available() {
            return None;
        }
        let (default_allowed, default_excluded) = config.domain_defaults();
        let http = supercharge_extra_ca::build_reqwest_client(|builder| {
            builder.timeout(SEARCH_HTTP_TIMEOUT)
        })
        .ok()?;
        Some(Self {
            responses,
            exa_key: config.exa_api_key(),
            linkup_key: config.linkup_api_key(),
            default_allowed,
            default_excluded,
            http,
            exa_origin: EXA_ORIGIN.to_string(),
            linkup_origin: LINKUP_ORIGIN.to_string(),
        })
    }

    pub fn with_origins(
        mut self,
        exa_origin: impl Into<String>,
        linkup_origin: impl Into<String>,
    ) -> Self {
        self.exa_origin = exa_origin.into();
        self.linkup_origin = linkup_origin.into();
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        if let Ok(http) =
            supercharge_extra_ca::build_reqwest_client(|builder| builder.timeout(timeout))
        {
            self.http = http;
        }
        self
    }

    pub fn uses_responses(&self) -> bool {
        self.exa_key.is_none() && self.linkup_key.is_none() && self.responses.is_some()
    }

    pub fn backend_name(&self) -> &'static str {
        if self.exa_key.is_some() {
            "exa"
        } else if self.linkup_key.is_some() {
            "linkup"
        } else if self.responses.is_some() {
            "default"
        } else {
            "none"
        }
    }

    fn resolve_filters(
        &self,
        model_allowed: Option<Vec<String>>,
    ) -> (Option<Vec<String>>, Option<Vec<String>>) {
        if let Some(allowed) = self.default_allowed.clone().filter(|d| !d.is_empty()) {
            return (Some(allowed), None);
        }
        if let Some(excluded) = self.default_excluded.clone().filter(|d| !d.is_empty()) {
            return (None, Some(excluded));
        }
        (model_allowed.filter(|d| !d.is_empty()), None)
    }

    pub async fn search(
        &self,
        query: &str,
        model_allowed: Option<Vec<String>>,
        mode: SearchMode,
        urls: Option<Vec<String>>,
    ) -> Result<(String, Vec<String>, Option<String>), supercharge_tool_runtime::ToolError> {
        let (allowed, excluded) = self.resolve_filters(model_allowed.clone());
        if let Some(urls) = urls.filter(|u| u.iter().any(|s| !s.trim().is_empty())) {
            return self.fetch_contents(&urls, allowed, excluded).await;
        }
        if self.exa_key.is_some() {
            match self
                .exa_search(query, mode, allowed.clone(), excluded.clone())
                .await
            {
                Ok(formatted) => return Ok(formatted),
                Err(exa_err) => {
                    tracing::warn!(
                        backend = "exa",
                        class = exa_err.class.as_str(),
                        "web search primary failed"
                    );
                    if self.linkup_key.is_some() {
                        return self
                            .linkup_search(query, mode, allowed, excluded)
                            .await
                            .map_err(|linkup_err| tool_err(format!("{exa_err}; {linkup_err}")));
                    }
                    return Err(tool_err(exa_err.to_string()));
                }
            }
        }
        if self.linkup_key.is_some() {
            return self
                .linkup_search(query, mode, allowed, excluded)
                .await
                .map_err(|e| tool_err(e.to_string()));
        }
        if let Some(client) = &self.responses {
            let (content, citations) = client.search(query, model_allowed).await?;
            return Ok((content, citations, None));
        }
        Err(tool_err("web_search is not configured".to_string()))
    }

    async fn fetch_contents(
        &self,
        urls: &[String],
        allowed: Option<Vec<String>>,
        excluded: Option<Vec<String>>,
    ) -> Result<(String, Vec<String>, Option<String>), supercharge_tool_runtime::ToolError> {
        let Some(key) = self.exa_key.as_deref() else {
            return Err(tool_err(
                "full page text needs an Exa key; web_fetch can still load a URL".to_string(),
            ));
        };
        let urls: Vec<&str> = urls
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        let body = json!({
            "urls": urls,
            "text": { "maxCharacters": PAGE_TEXT_CHARS },
        });
        let url = format!("{}/contents", self.exa_origin.trim_end_matches('/'));
        let value = self
            .post_json("Exa", &url, key, &body)
            .await
            .map_err(|e| tool_err(e.to_string()))?;
        let _ = (allowed, excluded);
        Ok(format_results(&value, "text", "highlights", "title"))
    }

    async fn exa_search(
        &self,
        query: &str,
        mode: SearchMode,
        allowed: Option<Vec<String>>,
        excluded: Option<Vec<String>>,
    ) -> Result<(String, Vec<String>, Option<String>), SearchFailure> {
        let key = self.exa_key.as_deref().unwrap_or("");
        let mut body = json!({
            "query": query,
            "type": mode.exa_type(),
            "numResults": MAX_RESULTS,
            "contents": {
                "text": { "maxCharacters": SNIPPET_CHARS },
                "highlights": { "maxCharacters": 1000 },
            },
        });
        insert_domains(&mut body, "includeDomains", allowed.as_ref());
        insert_domains(&mut body, "excludeDomains", excluded.as_ref());
        let url = format!("{}/search", self.exa_origin.trim_end_matches('/'));
        let value = self.post_json("Exa", &url, key, &body).await?;
        Ok(format_results(&value, "text", "highlights", "title"))
    }

    async fn linkup_search(
        &self,
        query: &str,
        mode: SearchMode,
        allowed: Option<Vec<String>>,
        excluded: Option<Vec<String>>,
    ) -> Result<(String, Vec<String>, Option<String>), SearchFailure> {
        let key = self.linkup_key.as_deref().unwrap_or("");
        let mut body = json!({
            "q": query,
            "depth": mode.linkup_depth(),
            "outputType": "searchResults",
            "maxResults": MAX_RESULTS,
        });
        insert_domains(&mut body, "includeDomains", allowed.as_ref());
        insert_domains(&mut body, "excludeDomains", excluded.as_ref());
        let url = format!("{}/v1/search", self.linkup_origin.trim_end_matches('/'));
        let value = self.post_json("Linkup", &url, key, &body).await?;
        Ok(format_results(&value, "content", "highlights", "name"))
    }

    async fn post_json(
        &self,
        backend: &'static str,
        url: &str,
        key: &str,
        body: &Value,
    ) -> Result<Value, SearchFailure> {
        let mut headers = HeaderMap::new();
        let bearer = match HeaderValue::from_str(&format!("Bearer {key}")) {
            Ok(v) => v,
            Err(_) => {
                return Err(SearchFailure::new(
                    backend,
                    FailureClass::Unauthorized,
                    "invalid api key",
                    &[key],
                ));
            }
        };
        headers.insert(AUTHORIZATION, bearer);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if backend == "Exa"
            && let Ok(api_key_header) = HeaderValue::from_str(key)
        {
            headers.insert("x-api-key", api_key_header);
        }
        let response = match self.http.post(url).headers(headers).json(body).send().await {
            Ok(response) => response,
            Err(err) => {
                let class = if err.is_timeout() {
                    FailureClass::Timeout
                } else {
                    FailureClass::Network
                };
                return Err(SearchFailure::new(backend, class, "", &[key]));
            }
        };
        let status = response.status();
        let bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(err) => {
                let class = if err.is_timeout() {
                    FailureClass::Timeout
                } else {
                    FailureClass::Network
                };
                return Err(SearchFailure::new(backend, class, "", &[key]));
            }
        };
        if !status.is_success() {
            let class = match status.as_u16() {
                401 | 403 => FailureClass::Unauthorized,
                402 => FailureClass::PaymentRequired,
                429 => FailureClass::RateLimited,
                400..=499 => FailureClass::BadRequest,
                _ => FailureClass::Server,
            };
            let raw = String::from_utf8_lossy(&bytes);
            let detail = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            return Err(SearchFailure::new(backend, class, detail, &[key]));
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| SearchFailure::new(backend, FailureClass::Unparseable, "", &[key]))
    }
}

fn tool_err(msg: String) -> supercharge_tool_runtime::ToolError {
    supercharge_tool_runtime::ToolError::execution(
        supercharge_tool_protocol::ToolId::new("web_search").expect("valid"),
        msg,
    )
}

fn insert_domains(body: &mut Value, field: &str, domains: Option<&Vec<String>>) {
    let Some(domains) = domains.filter(|d| !d.is_empty()) else {
        return;
    };
    if let Some(obj) = body.as_object_mut() {
        obj.insert(field.to_string(), json!(domains));
    }
}

fn trim_text(text: &str, cap: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= cap {
        return trimmed.to_string();
    }
    let end = trimmed
        .char_indices()
        .nth(cap.saturating_sub(1))
        .map(|(i, _)| i)
        .unwrap_or(trimmed.len());
    format!("{}…", &trimmed[..end])
}

fn result_list(value: &Value) -> Vec<Value> {
    value
        .get("results")
        .and_then(|v| v.as_array())
        .cloned()
        .or_else(|| value.as_array().cloned())
        .unwrap_or_default()
}

fn format_results(
    value: &Value,
    text_field: &str,
    highlights_field: &str,
    title_field: &str,
) -> (String, Vec<String>, Option<String>) {
    let mut blocks = Vec::new();
    let mut citations = Vec::new();
    for item in result_list(value) {
        let title = item
            .get(title_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let url = item
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if url.is_empty() {
            continue;
        }
        let mut text = item
            .get(text_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if text.trim().is_empty() {
            text = item
                .get(highlights_field)
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|h| h.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
        }
        let text = trim_text(&text, SNIPPET_CHARS);
        citations.push(url.to_string());
        blocks.push(format!("Title: {title}\nURL: {url}\nContent: {text}"));
    }
    let body = if blocks.is_empty() {
        "No search results found.".to_string()
    } else {
        blocks.join("\n---\n")
    };
    (body.clone(), citations, Some(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const EXA_KEY: &str = "exa-secret-KEY-xyz";
    const LINKUP_KEY: &str = "linkup-secret-KEY-xyz";

    fn providers(exa: Option<&str>, linkup: Option<&str>) -> WebSearchConfig {
        WebSearchConfig::Disabled.with_search_providers(
            false,
            exa.map(str::to_string),
            linkup.map(str::to_string),
            None,
            None,
        )
    }

    fn router_for(config: &WebSearchConfig, exa: &str, linkup: &str) -> WebSearchRouter {
        WebSearchRouter::from_config(config, None)
            .expect("router")
            .with_origins(exa, linkup)
            .with_timeout(Duration::from_millis(400))
    }

    fn exa_body() -> Value {
        json!({
            "results": [{
                "title": "Rust Release",
                "url": "https://example.com/rust",
                "text": "a".repeat(2500),
                "highlights": ["short"]
            }]
        })
    }

    fn linkup_body() -> Value {
        json!({
            "results": [{
                "name": "Linkup Hit",
                "url": "https://example.com/linkup",
                "content": "from linkup"
            }]
        })
    }

    fn assert_no_key(text: &str) {
        assert!(!text.contains(EXA_KEY), "{text}");
        assert!(!text.contains(LINKUP_KEY), "{text}");
        assert!(!text.contains("Bearer"), "{text}");
    }

    #[test]
    fn selector_keeps_responses_when_no_keys() {
        let config = WebSearchConfig::Enabled {
            api_key: "responses-key".into(),
            base_url: "http://127.0.0.1:9".into(),
            model: "m".into(),
            extra_headers: Default::default(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
            exa_api_key: None,
            linkup_api_key: None,
        };
        let client = WebSearchClient::new(&config, None).expect("client");
        let router = WebSearchRouter::from_config(&config, Some(client)).expect("router");
        assert!(router.uses_responses());
        assert_eq!(router.backend_name(), "default");
        assert!(!format!("{router:?}").contains("responses-key"));
    }

    #[test]
    fn selector_disabled_without_keys_is_absent() {
        assert!(WebSearchRouter::from_config(&WebSearchConfig::Disabled, None).is_none());
        let killed = WebSearchConfig::Disabled.with_search_providers(
            true,
            Some(EXA_KEY.into()),
            Some(LINKUP_KEY.into()),
            None,
            None,
        );
        assert!(matches!(killed, WebSearchConfig::Disabled));
        assert!(WebSearchRouter::from_config(&killed, None).is_none());
    }

    #[test]
    fn selector_linkup_only_skips_responses() {
        let responses = WebSearchConfig::Enabled {
            api_key: "responses-key".into(),
            base_url: "http://127.0.0.1:9".into(),
            model: "m".into(),
            extra_headers: Default::default(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
            exa_api_key: None,
            linkup_api_key: Some(LINKUP_KEY.into()),
        };
        let client = WebSearchClient::new(&responses, None).unwrap();
        let router = WebSearchRouter::from_config(&responses, Some(client)).unwrap();
        assert!(!router.uses_responses());
        assert_eq!(router.backend_name(), "linkup");
    }

    #[tokio::test]
    async fn exa_success_trims_text_and_hides_key() {
        let exa = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(exa_body()))
            .mount(&exa)
            .await;
        let router = router_for(
            &providers(Some(EXA_KEY), None),
            &exa.uri(),
            "http://127.0.0.1:9",
        );
        let (content, citations, pre) = router
            .search("rust", None, SearchMode::Auto, None)
            .await
            .unwrap();
        assert!(content.contains("Rust Release"));
        assert!(content.contains("https://example.com/rust"));
        assert!(content.contains('…'));
        assert!(!content.contains(&"a".repeat(2500)));
        assert_eq!(citations, vec!["https://example.com/rust".to_string()]);
        assert_no_key(&content);
        assert_no_key(pre.as_deref().unwrap_or(""));
        let req = exa.received_requests().await.unwrap();
        let body: Value = req[0].body_json().unwrap();
        assert_eq!(body["type"], "auto");
        assert_eq!(body["numResults"], 10);
        assert!(body["contents"]["text"].is_object());
        assert!(body.get("summary").is_none());
        let auth = req[0]
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(auth.starts_with("Bearer "));
        assert!(req[0].headers.get("x-api-key").is_some());
    }

    #[tokio::test]
    async fn lite_mode_sets_exa_type_and_linkup_depth() {
        let exa = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(exa_body()))
            .mount(&exa)
            .await;
        let router = router_for(
            &providers(Some(EXA_KEY), None),
            &exa.uri(),
            "http://127.0.0.1:9",
        );
        router
            .search("q", None, SearchMode::Lite, None)
            .await
            .unwrap();
        let req = exa.received_requests().await.unwrap();
        let body: Value = req[0].body_json().unwrap();
        assert_eq!(body["type"], "lite");
        assert!(body.get("contents").is_some());

        let linkup = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(linkup_body()))
            .mount(&linkup)
            .await;
        let router = router_for(
            &providers(None, Some(LINKUP_KEY)),
            "http://127.0.0.1:9",
            &linkup.uri(),
        );
        router
            .search("q", None, SearchMode::Lite, None)
            .await
            .unwrap();
        let req = linkup.received_requests().await.unwrap();
        let body: Value = req[0].body_json().unwrap();
        assert_eq!(body["depth"], "fast");
        assert_eq!(body["outputType"], "searchResults");
        assert_eq!(body["maxResults"], 10);
    }

    #[tokio::test]
    async fn missing_exa_uses_linkup_and_never_calls_exa() {
        let exa = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&exa)
            .await;
        let linkup = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(linkup_body()))
            .mount(&linkup)
            .await;
        let router = router_for(
            &providers(None, Some(LINKUP_KEY)),
            &exa.uri(),
            &linkup.uri(),
        );
        let (content, citations, _) = router
            .search("q", None, SearchMode::Auto, None)
            .await
            .unwrap();
        assert!(content.contains("Linkup Hit"));
        assert_eq!(citations, vec!["https://example.com/linkup".to_string()]);
        assert!(exa.received_requests().await.unwrap().is_empty());
        let req = linkup.received_requests().await.unwrap();
        let body: Value = req[0].body_json().unwrap();
        assert_eq!(body["depth"], "standard");
    }

    async fn fallback_on_exa_status(status: u16) {
        let exa = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(status).set_body_string(format!("nope {EXA_KEY}")))
            .mount(&exa)
            .await;
        let linkup = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(linkup_body()))
            .mount(&linkup)
            .await;
        let router = router_for(
            &providers(Some(EXA_KEY), Some(LINKUP_KEY)),
            &exa.uri(),
            &linkup.uri(),
        );
        let (content, _, _) = router
            .search("q", None, SearchMode::Auto, None)
            .await
            .unwrap();
        assert!(content.contains("from linkup"));
        assert_no_key(&content);
    }

    #[tokio::test]
    async fn exa_5xx_402_and_429_fall_back_to_linkup() {
        fallback_on_exa_status(500).await;
        fallback_on_exa_status(402).await;
        fallback_on_exa_status(429).await;
    }

    #[tokio::test]
    async fn exa_timeout_falls_back_to_linkup() {
        let exa = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(2))
                    .set_body_json(exa_body()),
            )
            .mount(&exa)
            .await;
        let linkup = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(linkup_body()))
            .mount(&linkup)
            .await;
        let router = router_for(
            &providers(Some(EXA_KEY), Some(LINKUP_KEY)),
            &exa.uri(),
            &linkup.uri(),
        );
        let (content, _, _) = router
            .search("q", None, SearchMode::Auto, None)
            .await
            .unwrap();
        assert!(content.contains("from linkup"));
    }

    #[tokio::test]
    async fn exa_unauthorized_falls_back_when_linkup_set() {
        fallback_on_exa_status(401).await;
        fallback_on_exa_status(403).await;
    }

    #[tokio::test]
    async fn exa_unauthorized_without_linkup_is_a_clear_error() {
        let exa = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(401).set_body_string(format!("bad {EXA_KEY}")))
            .mount(&exa)
            .await;
        let router = router_for(
            &providers(Some(EXA_KEY), None),
            &exa.uri(),
            "http://127.0.0.1:9",
        );
        let err = router
            .search("q", None, SearchMode::Auto, None)
            .await
            .unwrap_err();
        let text = format!("{err} {err:?}");
        assert!(text.to_lowercase().contains("unauthorized"), "{text}");
        assert!(text.contains("Exa"), "{text}");
        assert_no_key(&text);
    }

    #[tokio::test]
    async fn unparseable_exa_body_falls_back() {
        let exa = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .mount(&exa)
            .await;
        let linkup = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(linkup_body()))
            .mount(&linkup)
            .await;
        let router = router_for(
            &providers(Some(EXA_KEY), Some(LINKUP_KEY)),
            &exa.uri(),
            &linkup.uri(),
        );
        let (content, _, _) = router
            .search("q", None, SearchMode::Auto, None)
            .await
            .unwrap();
        assert!(content.contains("from linkup"));
    }

    #[tokio::test]
    async fn domain_filters_are_forwarded() {
        let exa = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(exa_body()))
            .mount(&exa)
            .await;
        let config = WebSearchConfig::Disabled.with_search_providers(
            false,
            Some(EXA_KEY.into()),
            None,
            Some(vec!["docs.example".into()]),
            Some(vec!["blocked.example".into()]),
        );
        let router = router_for(&config, &exa.uri(), "http://127.0.0.1:9");
        router
            .search(
                "q",
                Some(vec!["model.example".into()]),
                SearchMode::Auto,
                None,
            )
            .await
            .unwrap();
        let req = exa.received_requests().await.unwrap();
        let body: Value = req[0].body_json().unwrap();
        assert_eq!(body["includeDomains"], json!(["docs.example"]));
        assert!(body.get("excludeDomains").is_none());
    }

    #[tokio::test]
    async fn contents_endpoint_used_only_when_urls_given() {
        let exa = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/contents"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [{
                    "title": "Page",
                    "url": "https://example.com/page",
                    "text": "full page"
                }]
            })))
            .mount(&exa)
            .await;
        let router = router_for(
            &providers(Some(EXA_KEY), None),
            &exa.uri(),
            "http://127.0.0.1:9",
        );
        let (content, citations, _) = router
            .search(
                "ignored",
                None,
                SearchMode::Auto,
                Some(vec!["https://example.com/page".into()]),
            )
            .await
            .unwrap();
        assert!(content.contains("full page"));
        assert_eq!(citations, vec!["https://example.com/page".to_string()]);
        let req = exa.received_requests().await.unwrap();
        assert_eq!(req.len(), 1);
        assert_eq!(req[0].url.path(), "/contents");
    }
}
