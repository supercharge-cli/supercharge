//! Cursor-paginated sandbox listing.

use crate::api::client::ApiClient;
use crate::api::schema as api_schema;
use crate::connection_config::ConnectionConfig;
use crate::errors::Result;
use crate::paginator::PaginationState;
use crate::sandbox::opts::SandboxListOpts;
use crate::sandbox::types::{SandboxInfo, SandboxState};

/// A cursor-paginated listing of sandboxes (`GET /v2/sandboxes`).
pub struct SandboxPaginator {
    api: ApiClient,
    state: PaginationState,
    states: Vec<&'static str>,
    metadata: std::collections::BTreeMap<String, String>,
}

impl SandboxPaginator {
    /// Build a paginator from list options (validates the API key eagerly).
    pub(crate) fn new(opts: SandboxListOpts) -> Result<Self> {
        let config = ConnectionConfig::new(opts.connection);
        let api = ApiClient::new(&config, true)?;
        let states = opts
            .states
            .unwrap_or_else(|| vec![SandboxState::Running, SandboxState::Paused])
            .iter()
            .map(|s| match s {
                SandboxState::Running => "running",
                SandboxState::Paused => "paused",
            })
            .collect();
        Ok(Self {
            api,
            state: PaginationState::new(opts.limit, None),
            states,
            metadata: opts.metadata,
        })
    }

    /// Whether more pages remain.
    pub fn has_next(&self) -> bool {
        self.state.has_next()
    }

    /// Fetch the next page. Returns an empty vec (and stops) when exhausted.
    pub async fn next_items(&mut self) -> Result<Vec<SandboxInfo>> {
        if !self.state.has_next() {
            return Ok(Vec::new());
        }
        let mut query: Vec<(&str, String)> = Vec::new();
        // Control-plane arrays are form-style, NOT exploded: `state=running,paused`
        // (don't push repeated `state` pairs — reqwest would explode them).
        if !self.states.is_empty() {
            query.push(("state", self.states.join(",")));
        }
        if let Some(limit) = self.state.limit() {
            query.push(("limit", limit.to_string()));
        }
        if let Some(token) = self.state.next_token() {
            query.push(("nextToken", token.to_string()));
        }
        if !self.metadata.is_empty() {
            // metadata is a urlencoded `key=value&key2=value2` querystring (JS uses
            // URLSearchParams); reqwest url-encodes the whole value.
            let joined = self
                .metadata
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            query.push(("metadata", joined));
        }

        let (details, headers): (Vec<api_schema::SandboxDetail>, reqwest::header::HeaderMap) = self
            .api
            .request_with_headers(reqwest::Method::GET, "/v2/sandboxes", &query, None)
            .await?;

        let next = headers
            .get("x-next-token")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        self.state.update_from_token(next);

        Ok(details.into_iter().map(SandboxInfo::from_detail).collect())
    }
}

#[cfg(test)]
mod tests {
    use crate::sandbox::opts::SandboxListOpts;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn listed(id: &str) -> serde_json::Value {
        serde_json::json!({
            "sandboxID": id, "templateID": "base", "clientID": "c1",
            "cpuCount": 2, "memoryMB": 1024, "diskSizeMB": 1024,
            "envdVersion": "0.6.0", "state": "running",
            "startedAt": "2026-06-30T10:00:00Z", "endAt": "2026-06-30T10:05:00Z"
        })
    }

    #[tokio::test]
    async fn lists_one_page_and_reports_no_next() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/sandboxes"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([listed("sbx_a")])),
            )
            .mount(&server)
            .await;
        let opts = SandboxListOpts {
            connection: crate::connection_config::ConnectionConfigOpts {
                api_key: Some("e2b_0123456789abcdef".to_string()),
                api_url: Some(server.uri()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pager = crate::Sandbox::list(opts).expect("pager");
        assert!(pager.has_next()); // true before the first fetch
        let items = pager.next_items().await.expect("page");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].sandbox_id, "sbx_a");
        assert!(!pager.has_next()); // no x-next-token header -> done
    }
}
