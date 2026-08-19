//! Cursor-paginated snapshot listing.

use crate::api::client::ApiClient;
use crate::api::schema as api_schema;
use crate::connection_config::ConnectionConfig;
use crate::errors::Result;
use crate::paginator::PaginationState;
use crate::sandbox::opts::SnapshotListOpts;
use crate::sandbox::types::SnapshotInfo;

/// A cursor-paginated listing of snapshots (`GET /snapshots`).
pub struct SnapshotPaginator {
    api: ApiClient,
    state: PaginationState,
    sandbox_id: Option<String>,
}

impl SnapshotPaginator {
    /// Build a paginator from list options (validates the API key eagerly).
    pub(crate) fn new(opts: SnapshotListOpts) -> Result<Self> {
        let config = ConnectionConfig::new(opts.connection);
        let api = ApiClient::new(&config, true)?;
        Ok(Self {
            api,
            state: PaginationState::new(opts.limit, None),
            sandbox_id: opts.sandbox_id,
        })
    }

    /// Whether more pages remain.
    pub fn has_next(&self) -> bool {
        self.state.has_next()
    }

    /// Fetch the next page. Returns an empty vec (and stops) when exhausted.
    pub async fn next_items(&mut self) -> Result<Vec<SnapshotInfo>> {
        if !self.state.has_next() {
            return Ok(Vec::new());
        }
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(id) = &self.sandbox_id {
            query.push(("sandboxID", id.clone()));
        }
        if let Some(limit) = self.state.limit() {
            query.push(("limit", limit.to_string()));
        }
        if let Some(token) = self.state.next_token() {
            query.push(("nextToken", token.to_string()));
        }

        let (items, headers): (Vec<api_schema::SnapshotInfo>, reqwest::header::HeaderMap) = self
            .api
            .request_with_headers(reqwest::Method::GET, "/snapshots", &query, None)
            .await?;

        let next = headers
            .get("x-next-token")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        self.state.update_from_token(next);

        Ok(items.into_iter().map(SnapshotInfo::from_schema).collect())
    }
}

#[cfg(test)]
mod tests {
    use crate::sandbox::opts::SnapshotListOpts;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn lists_one_page_and_stops() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/snapshots"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "snapshotID": "snap_1", "names": ["a"] }
            ])))
            .mount(&server)
            .await;
        let opts = SnapshotListOpts {
            connection: crate::connection_config::ConnectionConfigOpts {
                api_key: Some("e2b_0123456789abcdef".to_string()),
                api_url: Some(server.uri()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pager = crate::Sandbox::list_snapshots(opts).expect("pager");
        assert!(pager.has_next());
        let items = pager.next_items().await.expect("page");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].snapshot_id, "snap_1");
        assert!(!pager.has_next());
    }
}
