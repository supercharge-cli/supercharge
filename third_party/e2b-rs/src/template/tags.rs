//! Tag operations, alias-existence checks, and a public `get_build_status` wrapper.
//!
//! Ports the following from the JS SDK:
//! - `getBuildStatus` (`index.ts`)
//! - `exists` / `aliasExists` — delegates to `checkAliasExists` in `buildApi.ts`
//! - `assignTags` / `removeTags` / `getTags` (`index.ts` + `buildApi.ts`)
//!
//! All operations are public associated functions on [`Template`].

use crate::api::client::ApiClient;
use crate::connection_config::{ConnectionConfig, ConnectionConfigOpts};
use crate::errors::{Error, Result};
use crate::template::builder::Template;
use crate::template::types::{TemplateBuildStatusResponse, TemplateTag};

// ─── TemplateApiOpts ─────────────────────────────────────────────────────────

/// Connection options for the tag/status/exists family of operations on
/// [`Template`].
///
/// All fields are optional; unset values fall back to environment-variable
/// resolution performed by [`crate::ConnectionConfig`].
///
/// # Secret handling
///
/// The `api_key` field is excluded from the `Debug` output — it is shown as
/// `<redacted>` to prevent accidental secret leakage in logs. This matches the
/// convention used by [`crate::template::BuildOptions`].
#[derive(Default, Clone)]
pub struct TemplateApiOpts {
    /// Override the E2B API key for this request.
    pub api_key: Option<String>,
    /// Override the E2B domain (e.g. `"e2b.app"`).
    pub domain: Option<String>,
    /// Override the API base URL.
    pub api_url: Option<String>,
    /// Per-request HTTP timeout in milliseconds.
    pub request_timeout_ms: Option<u64>,
}

impl std::fmt::Debug for TemplateApiOpts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TemplateApiOpts")
            .field(
                "api_key",
                if self.api_key.is_some() {
                    &"<redacted>"
                } else {
                    &"None"
                },
            )
            .field("domain", &self.domain)
            .field("api_url", &self.api_url)
            .field("request_timeout_ms", &self.request_timeout_ms)
            .finish()
    }
}

// ─── TemplateTagInfo ─────────────────────────────────────────────────────────

/// Information about the tags that were assigned to a template build.
///
/// Returned by [`Template::assign_tags`]. The `build_id` field is the
/// UUID (as a `String`) of the build to which the tags now point.
#[derive(Debug, Clone)]
pub struct TemplateTagInfo {
    /// String representation of the UUID identifying the build to which
    /// the tags were assigned.
    pub build_id: String,
    /// The tags that were assigned.
    pub tags: Vec<String>,
}

impl TemplateTagInfo {
    /// Converts the generated [`crate::api::schema::AssignedTemplateTags`]
    /// into a public [`TemplateTagInfo`].
    ///
    /// The generated `build_id` field is a [`uuid::Uuid`]; it is exposed
    /// here as a [`String`] via `.to_string()`.
    pub(crate) fn from_wire(w: crate::api::schema::AssignedTemplateTags) -> Self {
        Self {
            build_id: w.build_id.to_string(),
            tags: w.tags,
        }
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Build an [`ApiClient`] from [`TemplateApiOpts`].
fn build_api_client(opts: &TemplateApiOpts) -> Result<ApiClient> {
    let config = ConnectionConfig::new(ConnectionConfigOpts {
        api_key: opts.api_key.clone(),
        domain: opts.domain.clone(),
        api_url: opts.api_url.clone(),
        request_timeout_ms: opts.request_timeout_ms,
        ..Default::default()
    });
    ApiClient::new(&config, true)
}

// ─── Template impl ────────────────────────────────────────────────────────────

impl Template {
    /// Fetch the current build status for an existing template build.
    ///
    /// Builds a fresh API client from `opts` and polls the build-status
    /// endpoint at log offset 0.
    ///
    /// # Errors
    ///
    /// Returns an error if the API key is missing or invalid, or if the HTTP
    /// request fails.
    pub async fn get_build_status(
        template_id: &str,
        build_id: &str,
        opts: TemplateApiOpts,
    ) -> Result<TemplateBuildStatusResponse> {
        let api = build_api_client(&opts)?;
        crate::template::build_api::get_build_status(&api, template_id, build_id, 0).await
    }

    /// Check whether a template alias (or name) exists.
    ///
    /// Issues `GET /templates/aliases/{name}`:
    /// - `200` → `Ok(true)` — the alias exists.
    /// - `404` → `Ok(false)` — no such alias.
    /// - `403` → `Ok(true)` — the alias exists but is owned by another team
    ///   (matches the JS SDK's "exists but not yours" behavior).
    /// - Any other error is propagated as `Err`.
    ///
    /// # Errors
    ///
    /// Returns an error if the API key is missing, invalid, or the server
    /// returns a status other than 200/404/403.
    pub async fn exists(name: &str, opts: TemplateApiOpts) -> Result<bool> {
        let api = build_api_client(&opts)?;
        match api
            .request_unit(
                reqwest::Method::GET,
                &format!("/templates/aliases/{name}"),
                &[],
                None,
            )
            .await
        {
            Ok(()) => Ok(true),
            Err(Error::NotFound(_)) => Ok(false),
            // 403 means the alias resolves to a template owned by another team
            // — it exists, you just can't see it. Matches the JS SDK
            // (`checkAliasExists`: `if status === 403 return true`).
            Err(Error::Forbidden(_)) => Ok(true),
            Err(e) => Err(e),
        }
    }

    /// Check whether a template alias exists.
    ///
    /// This is a deprecated alias of [`Template::exists`] — prefer that
    /// method.
    ///
    /// Issues `GET /templates/aliases/{alias}`:
    /// - `200` → `Ok(true)`.
    /// - `404` → `Ok(false)`.
    /// - Any other error → propagated as `Err`.
    ///
    /// # Errors
    ///
    /// Returns an error if the API key is missing, invalid, or the server
    /// returns a non-200/404 status code.
    #[deprecated(note = "Use `Template::exists` instead")]
    pub async fn alias_exists(alias: &str, opts: TemplateApiOpts) -> Result<bool> {
        Template::exists(alias, opts).await
    }

    /// Assign tags to a template build.
    ///
    /// Issues `POST /templates/tags` with body
    /// `{"target": target_name, "tags": tags}`.
    ///
    /// `target_name` is in `"name:build_id"` format — the template name
    /// followed by a colon and the source build identifier (as accepted by
    /// the JS SDK's `assignTags`).
    ///
    /// Returns a [`TemplateTagInfo`] containing the build UUID (as a
    /// `String`) and the tags that were assigned.
    ///
    /// # Errors
    ///
    /// Returns an error if the API key is missing or invalid, or if the HTTP
    /// request fails.
    pub async fn assign_tags(
        target_name: &str,
        tags: &[String],
        opts: TemplateApiOpts,
    ) -> Result<TemplateTagInfo> {
        let api = build_api_client(&opts)?;
        let body = serde_json::json!({ "target": target_name, "tags": tags });
        let wire = api
            .request::<crate::api::schema::AssignedTemplateTags>(
                reqwest::Method::POST,
                "/templates/tags",
                &[],
                Some(&body),
            )
            .await?;
        Ok(TemplateTagInfo::from_wire(wire))
    }

    /// Remove tags from a template.
    ///
    /// Issues `DELETE /templates/tags` with body `{"name": name, "tags": tags}`.
    /// A successful response (`204 No Content`) returns `Ok(())`.
    ///
    /// # Errors
    ///
    /// Returns an error if the API key is missing or invalid, or if the HTTP
    /// request fails.
    pub async fn remove_tags(name: &str, tags: &[String], opts: TemplateApiOpts) -> Result<()> {
        let api = build_api_client(&opts)?;
        let body = serde_json::json!({ "name": name, "tags": tags });
        api.request_unit(reqwest::Method::DELETE, "/templates/tags", &[], Some(&body))
            .await
    }

    /// List all tags for a template.
    ///
    /// Issues `GET /templates/{template_id}/tags` and returns a
    /// [`Vec<TemplateTag>`] mapped from the internal wire type.
    ///
    /// # Errors
    ///
    /// Returns an error if the API key is missing or invalid, or if the HTTP
    /// request fails.
    pub async fn get_tags(template_id: &str, opts: TemplateApiOpts) -> Result<Vec<TemplateTag>> {
        let api = build_api_client(&opts)?;
        let path = format!("/templates/{template_id}/tags");
        let wire = api
            .request::<Vec<crate::api::schema::TemplateTag>>(reqwest::Method::GET, &path, &[], None)
            .await?;
        Ok(wire.into_iter().map(TemplateTag::from_wire).collect())
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_opts(server: &MockServer) -> TemplateApiOpts {
        TemplateApiOpts {
            api_key: Some("e2b_0123456789abcdef".to_string()),
            api_url: Some(server.uri()),
            ..Default::default()
        }
    }

    // ── exists_true_false_forbidden ───────────────────────────────────────────

    /// `exists` must return `true` for 200, `false` for 404, and `true` for 403
    /// (alias exists but owned by another team — matches the JS SDK).
    #[tokio::test]
    async fn exists_true_false_forbidden() {
        // ── 200 → true ──
        {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/templates/aliases/my-tpl"))
                .respond_with(ResponseTemplate::new(200))
                .mount(&server)
                .await;
            let result = Template::exists("my-tpl", test_opts(&server))
                .await
                .expect("exists must succeed on 200");
            assert!(result, "200 must map to true");
        }

        // ── 404 → false ──
        {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/templates/aliases/my-tpl"))
                .respond_with(
                    ResponseTemplate::new(404)
                        .set_body_json(serde_json::json!({ "code": 404, "message": "not found" })),
                )
                .mount(&server)
                .await;
            let result = Template::exists("my-tpl", test_opts(&server))
                .await
                .expect("exists must succeed on 404");
            assert!(!result, "404 must map to false");
        }

        // ── 403 → true (exists but not yours) ──
        {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/templates/aliases/my-tpl"))
                .respond_with(
                    ResponseTemplate::new(403)
                        .set_body_json(serde_json::json!({ "code": 403, "message": "forbidden" })),
                )
                .mount(&server)
                .await;
            let result = Template::exists("my-tpl", test_opts(&server))
                .await
                .expect("exists must succeed on 403");
            assert!(result, "403 must map to true (exists but not owner)");
        }
    }

    // ── assign_tags_posts ─────────────────────────────────────────────────────

    /// `assign_tags` must POST to `/templates/tags` with the correct body and
    /// map the response to [`TemplateTagInfo`].
    #[tokio::test]
    async fn assign_tags_posts() {
        let server = MockServer::start().await;
        let uuid_str = "550e8400-e29b-41d4-a716-446655440000";

        Mock::given(method("POST"))
            .and(path("/templates/tags"))
            .and(body_partial_json(
                serde_json::json!({ "target": "my-tpl", "tags": ["v1"] }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "buildID": uuid_str,
                "tags": ["v1"]
            })))
            .mount(&server)
            .await;

        let result = Template::assign_tags("my-tpl", &["v1".to_string()], test_opts(&server))
            .await
            .expect("assign_tags must succeed");

        assert_eq!(result.build_id, uuid_str);
        assert_eq!(result.tags, vec!["v1"]);
    }

    // ── remove_tags_deletes ───────────────────────────────────────────────────

    /// `remove_tags` must DELETE `/templates/tags` with the correct body and
    /// return `Ok(())` on 204.
    #[tokio::test]
    async fn remove_tags_deletes() {
        let server = MockServer::start().await;

        Mock::given(method("DELETE"))
            .and(path("/templates/tags"))
            .and(body_partial_json(
                serde_json::json!({ "name": "my-tpl", "tags": ["v1"] }),
            ))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        Template::remove_tags("my-tpl", &["v1".to_string()], test_opts(&server))
            .await
            .expect("remove_tags must succeed on 204");
    }

    // ── get_tags_maps ─────────────────────────────────────────────────────────

    /// `get_tags` must GET `/templates/{id}/tags`, decode the honest wire JSON,
    /// and map each entry via [`TemplateTag::from_wire`].
    #[tokio::test]
    async fn get_tags_maps() {
        let server = MockServer::start().await;
        let uuid_str = "550e8400-e29b-41d4-a716-446655440001";

        Mock::given(method("GET"))
            .and(path("/templates/tpl_1/tags"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "tag": "v1",
                    "buildID": uuid_str,
                    "createdAt": "2024-01-01T00:00:00Z"
                }
            ])))
            .mount(&server)
            .await;

        let tags = Template::get_tags("tpl_1", test_opts(&server))
            .await
            .expect("get_tags must succeed");

        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].tag, "v1");
        assert_eq!(tags[0].build_id, uuid_str);
    }

    // ── get_build_status_wraps ────────────────────────────────────────────────

    /// `Template::get_build_status` must delegate to
    /// `build_api::get_build_status` at log-offset 0 and return the mapped
    /// [`TemplateBuildStatusResponse`].
    #[tokio::test]
    async fn get_build_status_wraps() {
        use crate::template::types::BuildStatus;
        use wiremock::matchers::query_param;

        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/templates/tpl_1/builds/bld_1/status"))
            .and(query_param("logsOffset", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "templateID": "tpl_1",
                "buildID": "bld_1",
                "status": "ready",
                "logs": [],
                "logEntries": []
            })))
            .mount(&server)
            .await;

        let resp = Template::get_build_status("tpl_1", "bld_1", test_opts(&server))
            .await
            .expect("get_build_status must succeed");

        assert_eq!(resp.status, BuildStatus::Ready);
        assert_eq!(resp.template_id, "tpl_1");
        assert_eq!(resp.build_id, "bld_1");
    }

    // ── secret redaction ──────────────────────────────────────────────────────

    /// `api_key` must not appear in the `Debug` output of [`TemplateApiOpts`].
    #[test]
    fn template_api_opts_debug_redacts_api_key() {
        let opts = TemplateApiOpts {
            api_key: Some("e2b_super_secret".to_string()),
            domain: Some("e2b.app".to_string()),
            ..Default::default()
        };
        let debug_str = format!("{opts:?}");
        assert!(
            !debug_str.contains("e2b_super_secret"),
            "api_key must not appear in Debug: {debug_str}"
        );
        assert!(
            debug_str.contains("<redacted>"),
            "Debug must contain '<redacted>': {debug_str}"
        );
        assert!(
            debug_str.contains("e2b.app"),
            "domain should appear in Debug: {debug_str}"
        );
    }
}
