//! Public [`Volume`] struct and control-plane CRUD.
//!
//! The control-plane methods ([`Volume::create`], [`Volume::list`],
//! [`Volume::get_info`], [`Volume::connect`], [`Volume::destroy`]) talk to the
//! E2B API service via the internal `ApiClient`.
//!
//! Volume *content* operations (reading/writing files) are added in Tasks 3–4
//! and build a `VolumeApiClient` from the stored connection parameters.

use crate::api::client::ApiClient;
use crate::api::schema as api_schema;
use crate::connection_config::{ConnectionConfig, ConnectionConfigOpts, REQUEST_TIMEOUT_MS};
use crate::errors::{Error, Result};
use crate::volume::types::{
    VolumeAndToken, VolumeEntryStat, VolumeInfo, VolumeListOpts, VolumeMakeDirOpts,
    VolumeMetadataOpts, VolumeReadOpts, VolumeWriteOpts,
};

/// Default per-request timeout stored in [`Volume`] when
/// [`VolumeOpts::request_timeout_ms`] is `None`.
const DEFAULT_REQUEST_TIMEOUT_MS: u64 = REQUEST_TIMEOUT_MS;

/// Options for authenticating and connecting to the E2B volume API.
///
/// All fields are optional and fall back to environment variables where
/// documented. This mirrors the JS `VolumeApiOpts` type.
#[derive(Default, Clone)]
pub struct VolumeOpts {
    /// E2B API key. Falls back to the `E2B_API_KEY` environment variable.
    pub api_key: Option<String>,
    /// E2B domain override. Falls back to `E2B_DOMAIN` (default `e2b.app`).
    pub domain: Option<String>,
    /// Control-plane API base URL override. Falls back to `E2B_API_URL`.
    /// Primarily used in tests to point at a local or mock server.
    pub api_url: Option<String>,
    /// Per-request timeout in milliseconds applied to **both** control-plane
    /// calls (create/list/get_info/connect/destroy) and the volume-content
    /// client constructed in Tasks 3–4. Defaults to
    /// [`crate::connection_config::REQUEST_TIMEOUT_MS`] (60 s).
    pub request_timeout_ms: Option<u64>,
    /// Optional HTTP/HTTPS proxy URL forwarded to **both** the control-plane
    /// client and the volume-content client.
    pub proxy: Option<String>,
}

impl std::fmt::Debug for VolumeOpts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VolumeOpts")
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
            .field(
                "proxy",
                if self.proxy.is_some() {
                    &"<redacted>"
                } else {
                    &"None"
                },
            )
            .finish()
    }
}

/// A handle to an E2B persistent volume.
///
/// Holds the resolved connection parameters used to build a content client
/// (`VolumeApiClient`) in Tasks 3–4.
///
/// Obtain a `Volume` via [`Volume::create`] or [`Volume::connect`].
#[derive(Clone)]
pub struct Volume {
    volume_id: String,
    name: String,
    token: String,
    /// Resolved API/content base URL used to build [`crate::volume::client::VolumeApiClient`].
    api_url: String,
    /// Per-request timeout passed to [`crate::volume::client::VolumeApiClient`].
    request_timeout_ms: u64,
    /// Optional HTTP/HTTPS proxy URL passed to [`crate::volume::client::VolumeApiClient`].
    proxy: Option<String>,
}

impl std::fmt::Debug for Volume {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Volume")
            .field("volume_id", &self.volume_id)
            .field("name", &self.name)
            .field("token", &"<redacted>")
            .field("api_url", &self.api_url)
            .field("request_timeout_ms", &self.request_timeout_ms)
            .field(
                "proxy",
                if self.proxy.is_some() {
                    &"<redacted>"
                } else {
                    &"None"
                },
            )
            .finish()
    }
}

impl Volume {
    /// The unique identifier for this volume.
    pub fn volume_id(&self) -> &str {
        &self.volume_id
    }

    /// The human-readable name of this volume.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Short-lived Bearer token for the volume content API.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Construct a `Volume` from resolved parts.
    ///
    /// Used internally by [`Volume::create`] and [`Volume::connect`], and by
    /// integration tests that inject a wiremock base URL via `api_url`.
    pub(crate) fn from_parts(
        volume_id: String,
        name: String,
        token: String,
        api_url: String,
        request_timeout_ms: u64,
        proxy: Option<String>,
    ) -> Volume {
        Volume {
            volume_id,
            name,
            token,
            api_url,
            request_timeout_ms,
            proxy,
        }
    }

    /// Build a transient [`ApiClient`] and a resolved [`ConnectionConfig`] from
    /// the caller's [`VolumeOpts`].
    ///
    /// Both `proxy` and `request_timeout_ms` from [`VolumeOpts`] are forwarded
    /// here so that all control-plane calls honour them.
    fn build_api_client(opts: &VolumeOpts) -> Result<(ApiClient, ConnectionConfig)> {
        let config = ConnectionConfig::new(ConnectionConfigOpts {
            api_key: opts.api_key.clone(),
            domain: opts.domain.clone(),
            api_url: opts.api_url.clone(),
            proxy: opts.proxy.clone(),
            request_timeout_ms: opts.request_timeout_ms,
            ..Default::default()
        });
        let api = ApiClient::new(&config, true)?;
        Ok((api, config))
    }

    /// Create a new volume.
    ///
    /// Sends `POST /volumes` with the given `name` as the request body.
    /// `name` must match the pattern `^[a-zA-Z0-9_-]+$`.
    ///
    /// Returns a [`Volume`] handle that can be used to construct a content
    /// client in Tasks 3–4.
    pub async fn create(name: &str, opts: VolumeOpts) -> Result<Volume> {
        let (api, config) = Volume::build_api_client(&opts)?;
        let request_timeout_ms = opts
            .request_timeout_ms
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_MS);

        let volume_name = api_schema::NewVolumeName::try_from(name.to_string())
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        let body = serde_json::to_value(api_schema::NewVolume { name: volume_name })
            .map_err(|e| Error::Internal(e.to_string()))?;

        let res: api_schema::VolumeAndToken = api
            .request(reqwest::Method::POST, "/volumes", &[], Some(&body))
            .await?;

        Ok(Volume::from_parts(
            res.volume_id,
            res.name,
            res.token,
            config.api_url,
            request_timeout_ms,
            opts.proxy,
        ))
    }

    /// List all volumes accessible with the configured API key.
    ///
    /// Sends `GET /volumes` and maps each entry to a public [`VolumeInfo`].
    pub async fn list(opts: VolumeOpts) -> Result<Vec<VolumeInfo>> {
        let (api, _config) = Volume::build_api_client(&opts)?;
        let volumes: Vec<api_schema::Volume> = api
            .request(reqwest::Method::GET, "/volumes", &[], None)
            .await?;
        Ok(volumes.into_iter().map(VolumeInfo::from_wire).collect())
    }

    /// Fetch detailed information (including a fresh bearer token) for a volume.
    ///
    /// Sends `GET /volumes/{volume_id}` and returns a [`VolumeAndToken`]
    /// containing the volume metadata and a short-lived Bearer token for the
    /// content API.
    ///
    /// # Errors
    ///
    /// Returns [`crate::errors::Error::NotFound`] with the message
    /// `"Volume {volume_id} not found"` on HTTP 404, mirroring the JS SDK's
    /// `NotFoundError` message.
    pub async fn get_info(volume_id: &str, opts: VolumeOpts) -> Result<VolumeAndToken> {
        let (api, _config) = Volume::build_api_client(&opts)?;
        let path = format!("/volumes/{volume_id}");
        let res: api_schema::VolumeAndToken = api
            .request(reqwest::Method::GET, &path, &[], None)
            .await
            .map_err(|e| match e {
                Error::NotFound(_) => Error::NotFound(format!("Volume {volume_id} not found")),
                other => other,
            })?;
        Ok(VolumeAndToken::from_wire(res))
    }

    /// Connect to an existing volume by ID.
    ///
    /// Sends `GET /volumes/{volume_id}` to obtain a fresh bearer token and
    /// builds a [`Volume`] handle from the result.
    ///
    /// # Errors
    ///
    /// Returns [`crate::errors::Error::NotFound`] with the message
    /// `"Volume {volume_id} not found"` on HTTP 404, mirroring the JS SDK's
    /// `NotFoundError` message.
    pub async fn connect(volume_id: &str, opts: VolumeOpts) -> Result<Volume> {
        let (api, config) = Volume::build_api_client(&opts)?;
        let request_timeout_ms = opts
            .request_timeout_ms
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_MS);
        let path = format!("/volumes/{volume_id}");
        let res: api_schema::VolumeAndToken = api
            .request(reqwest::Method::GET, &path, &[], None)
            .await
            .map_err(|e| match e {
                Error::NotFound(_) => Error::NotFound(format!("Volume {volume_id} not found")),
                other => other,
            })?;
        Ok(Volume::from_parts(
            volume_id.to_string(),
            res.name,
            res.token,
            config.api_url,
            request_timeout_ms,
            opts.proxy,
        ))
    }

    /// Destroy a volume.
    ///
    /// Sends `DELETE /volumes/{volume_id}`.
    ///
    /// Returns `true` if the volume was deleted, `false` if it was not found
    /// (already gone), mirroring the JS SDK's "already gone → false" behaviour.
    pub async fn destroy(volume_id: &str, opts: VolumeOpts) -> Result<bool> {
        let (api, _config) = Volume::build_api_client(&opts)?;
        let path = format!("/volumes/{volume_id}");
        match api
            .request_unit(reqwest::Method::DELETE, &path, &[], None)
            .await
        {
            Ok(()) => Ok(true),
            Err(Error::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

// ── Volume-content metadata operations ──────────────────────────────────────

/// Private JSON body for `PATCH /volumecontent/{id}/path`.
///
/// Fields are omitted from serialization when `None` so that callers only
/// send the metadata they want to update, mirroring the JS `updateMetadata`
/// body shape.
#[derive(serde::Serialize)]
struct MetadataBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    uid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<u32>,
}

impl Volume {
    /// Build a [`crate::volume::client::VolumeApiClient`] from this handle's
    /// stored connection parameters.
    fn build_content_client(&self) -> Result<crate::volume::client::VolumeApiClient> {
        crate::volume::client::VolumeApiClient::new(
            &self.api_url,
            &self.token,
            self.request_timeout_ms,
            self.proxy.as_deref(),
        )
    }

    /// List the entries of the directory at `path`.
    ///
    /// Named `list_dir` to avoid clashing with the control-plane
    /// [`Volume::list`] associated function — Rust forbids a same-named
    /// associated function and instance method in one `impl` block.
    ///
    /// Mirrors JS `volume.list(path, opts)`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::errors::Error::NotFound`] with the message
    /// `"Path {path} not found"` on HTTP 404, mirroring the JS error message.
    pub async fn list_dir(&self, path: &str, opts: VolumeListOpts) -> Result<Vec<VolumeEntryStat>> {
        let client = self.build_content_client()?;
        let endpoint = format!("/volumecontent/{}/dir", self.volume_id);
        let mut query: Vec<(&str, String)> = vec![("path", path.to_string())];
        if let Some(d) = opts.depth {
            query.push(("depth", d.to_string()));
        }
        let listing: crate::volume::schema::VolumeDirectoryListing = client
            .request_json(reqwest::Method::GET, &endpoint, &query, None::<&()>)
            .await
            .map_err(|e| match e {
                Error::NotFound(_) => Error::NotFound(format!("Path {path} not found")),
                other => other,
            })?;
        Ok(listing
            .0
            .into_iter()
            .map(VolumeEntryStat::from_wire)
            .collect())
    }

    /// Create a directory at `path`.
    ///
    /// Mirrors JS `volume.makeDir(path, opts)`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::errors::Error::NotFound`] with the message
    /// `"Path {path} not found"` on HTTP 404, mirroring the JS error message.
    pub async fn make_dir(&self, path: &str, opts: VolumeMakeDirOpts) -> Result<VolumeEntryStat> {
        let client = self.build_content_client()?;
        let endpoint = format!("/volumecontent/{}/dir", self.volume_id);
        let mut query: Vec<(&str, String)> = vec![("path", path.to_string())];
        if let Some(u) = opts.uid {
            query.push(("uid", u.to_string()));
        }
        if let Some(g) = opts.gid {
            query.push(("gid", g.to_string()));
        }
        if let Some(m) = opts.mode {
            query.push(("mode", m.to_string()));
        }
        if let Some(f) = opts.force {
            query.push(("force", if f { "true" } else { "false" }.to_string()));
        }
        let stat: crate::volume::schema::VolumeEntryStat = client
            .request_json(reqwest::Method::POST, &endpoint, &query, None::<&()>)
            .await
            .map_err(|e| match e {
                Error::NotFound(_) => Error::NotFound(format!("Path {path} not found")),
                other => other,
            })?;
        Ok(VolumeEntryStat::from_wire(stat))
    }

    /// Return metadata for the file or directory at `path`.
    ///
    /// Named `stat` to avoid clashing with the control-plane
    /// [`Volume::get_info`] associated function.
    ///
    /// Mirrors JS `volume.getInfo(path)`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::errors::Error::NotFound`] with the message
    /// `"Path {path} not found"` on HTTP 404, mirroring the JS error message.
    pub async fn stat(&self, path: &str) -> Result<VolumeEntryStat> {
        let client = self.build_content_client()?;
        let endpoint = format!("/volumecontent/{}/path", self.volume_id);
        let query: Vec<(&str, String)> = vec![("path", path.to_string())];
        let stat: crate::volume::schema::VolumeEntryStat = client
            .request_json(reqwest::Method::GET, &endpoint, &query, None::<&()>)
            .await
            .map_err(|e| match e {
                Error::NotFound(_) => Error::NotFound(format!("Path {path} not found")),
                other => other,
            })?;
        Ok(VolumeEntryStat::from_wire(stat))
    }

    /// Update the metadata (uid / gid / mode) of the entry at `path`.
    ///
    /// Only fields set in `metadata` are sent in the request body; omitted
    /// fields are left unchanged on the server.
    ///
    /// Mirrors JS `volume.updateMetadata(path, metadata)`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::errors::Error::NotFound`] with the message
    /// `"Path {path} not found"` on HTTP 404, mirroring the JS error message.
    pub async fn update_metadata(
        &self,
        path: &str,
        metadata: VolumeMetadataOpts,
    ) -> Result<VolumeEntryStat> {
        let client = self.build_content_client()?;
        let endpoint = format!("/volumecontent/{}/path", self.volume_id);
        let query: Vec<(&str, String)> = vec![("path", path.to_string())];
        let body = MetadataBody {
            uid: metadata.uid,
            gid: metadata.gid,
            mode: metadata.mode,
        };
        let stat: crate::volume::schema::VolumeEntryStat = client
            .request_json(reqwest::Method::PATCH, &endpoint, &query, Some(&body))
            .await
            .map_err(|e| match e {
                Error::NotFound(_) => Error::NotFound(format!("Path {path} not found")),
                other => other,
            })?;
        Ok(VolumeEntryStat::from_wire(stat))
    }

    /// Remove the file or directory at `path`.
    ///
    /// Mirrors JS `volume.remove(path)`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::errors::Error::NotFound`] with the message
    /// `"Path {path} not found"` on HTTP 404, mirroring the JS error message.
    pub async fn remove(&self, path: &str) -> Result<()> {
        let client = self.build_content_client()?;
        let endpoint = format!("/volumecontent/{}/path", self.volume_id);
        let query: Vec<(&str, String)> = vec![("path", path.to_string())];
        client
            .request_unit(reqwest::Method::DELETE, &endpoint, &query, None::<&()>)
            .await
            .map_err(|e| match e {
                Error::NotFound(_) => Error::NotFound(format!("Path {path} not found")),
                other => other,
            })
    }

    /// Return `true` if the path exists in the volume, `false` if it does not.
    ///
    /// Calls [`Volume::stat`] internally; HTTP 404 maps to `Ok(false)` and any
    /// other error is propagated. Mirrors JS `volume.exists(path)`.
    pub async fn exists(&self, path: &str) -> Result<bool> {
        match self.stat(path).await {
            Ok(_) => Ok(true),
            Err(Error::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

// ── Volume-content file I/O ──────────────────────────────────────────────────

impl Volume {
    /// Build a [`crate::volume::client::VolumeApiClient`] with the 1-hour file
    /// timeout.
    ///
    /// File read/write operations can take a long time for large files, so
    /// they must use [`crate::volume::client::FILE_TIMEOUT_MS`] (1 hour) rather
    /// than the 60-second metadata default stored in `self.request_timeout_ms`.
    /// This mirrors the JS SDK's `readFile`/`writeFile` which pass
    /// `requestTimeoutMs: opts?.requestTimeoutMs ?? FILE_TIMEOUT_MS`.
    fn build_file_client(&self) -> Result<crate::volume::client::VolumeApiClient> {
        crate::volume::client::VolumeApiClient::new(
            &self.api_url,
            &self.token,
            crate::volume::client::FILE_TIMEOUT_MS,
            self.proxy.as_deref(),
        )
    }

    /// Read the file at `path` and return its contents as a strict UTF-8 `String`.
    ///
    /// Sends `GET /volumecontent/{id}/file` with `path` as a query parameter.
    /// Uses the 1-hour file timeout (mirrors JS `readFile(path)` / `format: 'text'`).
    ///
    /// # Errors
    ///
    /// Returns [`crate::errors::Error::NotFound`] with the message
    /// `"Path {path} not found"` on HTTP 404.
    ///
    /// Returns [`crate::errors::Error::Internal`] if the file bytes are not valid
    /// UTF-8. This is a documented divergence from the JS SDK, whose Fetch
    /// `.text()` decodes UTF-8 *lossily* (replacing invalid sequences with U+FFFD).
    /// Use [`Self::read_file_bytes`] for non-UTF-8 or binary content.
    pub async fn read_file(&self, path: &str) -> Result<String> {
        let client = self.build_file_client()?;
        let endpoint = format!("/volumecontent/{}/file", self.volume_id);
        let query: Vec<(&str, String)> = vec![("path", path.to_string())];
        let bytes = client
            .read_bytes(&endpoint, &query)
            .await
            .map_err(|e| match e {
                Error::NotFound(_) => Error::NotFound(format!("Path {path} not found")),
                other => other,
            })?;
        String::from_utf8(bytes)
            .map_err(|e| Error::Internal(format!("volume file is not valid UTF-8: {e}")))
    }

    /// Read the raw bytes of the file at `path`.
    ///
    /// Sends `GET /volumecontent/{id}/file` with `path` as a query parameter.
    /// Uses the 1-hour file timeout (mirrors JS `readFile(path, { format: 'bytes' })`).
    ///
    /// # Errors
    ///
    /// Returns [`crate::errors::Error::NotFound`] with the message
    /// `"Path {path} not found"` on HTTP 404.
    pub async fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let client = self.build_file_client()?;
        let endpoint = format!("/volumecontent/{}/file", self.volume_id);
        let query: Vec<(&str, String)> = vec![("path", path.to_string())];
        client
            .read_bytes(&endpoint, &query)
            .await
            .map_err(|e| match e {
                Error::NotFound(_) => Error::NotFound(format!("Path {path} not found")),
                other => other,
            })
    }

    /// Stream the bytes of the file at `path`.
    ///
    /// Sends `GET /volumecontent/{id}/file` with `path` as a query parameter
    /// and returns a byte stream. The idle timeout defaults to the 1-hour file
    /// timeout and can be overridden via [`VolumeReadOpts::stream_idle_timeout_ms`].
    ///
    /// Mirrors JS `readFile(path, { format: 'stream' })`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::errors::Error::NotFound`] with the message
    /// `"Path {path} not found"` on HTTP 404.
    pub async fn read_file_stream(
        &self,
        path: &str,
        opts: VolumeReadOpts,
    ) -> Result<impl futures::Stream<Item = Result<bytes::Bytes>>> {
        let client = self.build_file_client()?;
        let endpoint = format!("/volumecontent/{}/file", self.volume_id);
        let query: Vec<(&str, String)> = vec![("path", path.to_string())];
        let idle_timeout = opts
            .stream_idle_timeout_ms
            .unwrap_or(crate::volume::client::FILE_TIMEOUT_MS);
        client
            .read_stream(&endpoint, &query, idle_timeout)
            .await
            .map_err(|e| match e {
                Error::NotFound(_) => Error::NotFound(format!("Path {path} not found")),
                other => other,
            })
    }

    /// Write `data` to the file at `path`, creating or overwriting it.
    ///
    /// Sends `PUT /volumecontent/{id}/file` with `path` and optional ownership /
    /// permission query parameters. The body is sent as `application/octet-stream`.
    /// Uses the 1-hour file timeout (mirrors JS `writeFile(path, data, opts)`).
    ///
    /// Returns the [`VolumeEntryStat`] of the written file.
    ///
    /// # Errors
    ///
    /// Returns [`crate::errors::Error::NotFound`] with the message
    /// `"Path {path} not found"` on HTTP 404.
    pub async fn write_file(
        &self,
        path: &str,
        data: impl Into<Vec<u8>>,
        opts: VolumeWriteOpts,
    ) -> Result<VolumeEntryStat> {
        let client = self.build_file_client()?;
        let endpoint = format!("/volumecontent/{}/file", self.volume_id);
        let mut query: Vec<(&str, String)> = vec![("path", path.to_string())];
        if let Some(u) = opts.uid {
            query.push(("uid", u.to_string()));
        }
        if let Some(g) = opts.gid {
            query.push(("gid", g.to_string()));
        }
        if let Some(m) = opts.mode {
            query.push(("mode", m.to_string()));
        }
        if let Some(f) = opts.force {
            query.push(("force", if f { "true" } else { "false" }.to_string()));
        }
        let stat: crate::volume::schema::VolumeEntryStat = client
            .write_bytes(&endpoint, &query, data.into())
            .await
            .map_err(|e| match e {
                Error::NotFound(_) => Error::NotFound(format!("Path {path} not found")),
                other => other,
            })?;
        Ok(VolumeEntryStat::from_wire(stat))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Build a [`VolumeOpts`] pointing at the wiremock server with a valid
    /// test API key (mirrors `api_for` in `sandbox::api` tests).
    fn opts_for(server: &MockServer) -> VolumeOpts {
        VolumeOpts {
            api_key: Some("e2b_0123456789abcdef".to_string()),
            api_url: Some(server.uri()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn create_posts_and_builds_volume() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/volumes"))
            .and(header("X-API-KEY", "e2b_0123456789abcdef"))
            .and(body_partial_json(serde_json::json!({ "name": "v" })))
            .respond_with(ResponseTemplate::new(201).set_body_json(
                serde_json::json!({ "volumeID": "vol_1", "name": "v", "token": "tkn" }),
            ))
            .mount(&server)
            .await;

        let vol = Volume::create("v", opts_for(&server))
            .await
            .expect("create ok");
        assert_eq!(vol.volume_id(), "vol_1");
        assert_eq!(vol.token(), "tkn");
        // api_url is not exposed via a getter, but we can verify the volume was
        // built by checking the public fields that rely on it being set correctly.
        assert_eq!(vol.name(), "v");
    }

    #[tokio::test]
    async fn create_rejects_invalid_name() {
        // Names with spaces or special chars violate `^[a-zA-Z0-9_-]+$`.
        let server = MockServer::start().await;
        let err = Volume::create("bad name!", opts_for(&server))
            .await
            .expect_err("invalid name should fail");
        assert!(matches!(err, Error::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn list_maps_to_info() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumes"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{ "volumeID": "vol_1", "name": "v" }])),
            )
            .mount(&server)
            .await;

        let infos = Volume::list(opts_for(&server)).await.expect("list ok");
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].volume_id, "vol_1");
        assert_eq!(infos[0].name, "v");
    }

    #[tokio::test]
    async fn get_info_returns_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumes/vol_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "volumeID": "vol_1", "name": "v", "token": "tkn" }),
            ))
            .mount(&server)
            .await;

        let info = Volume::get_info("vol_1", opts_for(&server))
            .await
            .expect("get_info ok");
        assert_eq!(info.volume_id, "vol_1");
        assert_eq!(info.name, "v");
        assert_eq!(info.token, "tkn");
    }

    #[tokio::test]
    async fn connect_builds_volume_from_get_info() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumes/vol_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "volumeID": "vol_1", "name": "v", "token": "tkn" }),
            ))
            .mount(&server)
            .await;

        let vol = Volume::connect("vol_1", opts_for(&server))
            .await
            .expect("connect ok");
        assert_eq!(vol.volume_id(), "vol_1");
        assert_eq!(vol.name(), "v");
        assert_eq!(vol.token(), "tkn");
    }

    #[tokio::test]
    async fn destroy_404_is_false() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/volumes/vol_x"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let gone = Volume::destroy("vol_x", opts_for(&server))
            .await
            .expect("destroy ok (idempotent)");
        assert!(!gone);
    }

    #[tokio::test]
    async fn destroy_204_is_true() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/volumes/vol_ok"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let deleted = Volume::destroy("vol_ok", opts_for(&server))
            .await
            .expect("destroy ok");
        assert!(deleted);
    }

    /// Verify that setting `VolumeOpts.proxy` flows into the control-plane
    /// `ConnectionConfigOpts` without error.  We use an unreachable proxy URL
    /// (`http://127.0.0.1:9`) which is syntactically valid — `build_api_client`
    /// must succeed even though no actual proxy is listening at that address
    /// (the proxy is only dialled when a real HTTP request is made, not at
    /// client construction time).
    #[test]
    fn build_api_client_with_proxy_succeeds() {
        let opts = VolumeOpts {
            api_key: Some("e2b_0123456789abcdef".to_string()),
            api_url: Some("http://127.0.0.1:9".to_string()),
            proxy: Some("http://127.0.0.1:9".to_string()),
            ..Default::default()
        };
        // `build_api_client` must succeed: an invalid proxy URL would return
        // `Err(Error::InvalidArgument(...))` here, so any success confirms the
        // proxy field was accepted by `reqwest::ClientBuilder`.
        Volume::build_api_client(&opts).expect("client builds with proxy set");
    }

    // ── Content-method tests ─────────────────────────────────────────────────

    use crate::volume::types::{
        VolumeFileType, VolumeListOpts, VolumeMakeDirOpts, VolumeMetadataOpts, VolumeReadOpts,
        VolumeWriteOpts,
    };
    use wiremock::matchers::{body_bytes, query_param};

    /// Build a [`Volume`] pointing at the wiremock server with a test Bearer
    /// token, bypassing the control-plane API entirely.
    fn vol_for(server: &MockServer) -> Volume {
        Volume::from_parts(
            "vol_1".to_string(),
            "test-volume".to_string(),
            "tkn".to_string(),
            server.uri(),
            5_000,
            None,
        )
    }

    /// Returns a minimal honest `VolumeEntryStat` JSON fixture.
    /// `"type"` is **lowercase** as returned by the real API.
    fn entry_json(name: &str, path: &str, file_type: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "path": path,
            "size": 0_i64,
            "mode": 420_u32,
            "uid": 1000_u32,
            "gid": 0_u32,
            "type": file_type,
            "atime": "2024-01-01T00:00:00Z",
            "mtime": "2024-01-01T00:00:00Z",
            "ctime": "2024-01-01T00:00:00Z"
        })
    }

    #[tokio::test]
    async fn list_dir_returns_entries() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumecontent/vol_1/dir"))
            .and(query_param("path", "/"))
            .and(header("Authorization", "Bearer tkn"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                entry_json("a.txt", "/a.txt", "file"),
                entry_json("subdir", "/subdir", "directory"),
            ])))
            .mount(&server)
            .await;

        let vol = vol_for(&server);
        let entries = vol
            .list_dir("/", VolumeListOpts::default())
            .await
            .expect("list_dir ok");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].file_type, VolumeFileType::File);
        assert_eq!(entries[1].file_type, VolumeFileType::Directory);
    }

    #[tokio::test]
    async fn make_dir_creates() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/volumecontent/vol_1/dir"))
            .and(query_param("path", "/new-dir"))
            .and(query_param("uid", "1000"))
            .and(header("Authorization", "Bearer tkn"))
            .respond_with(ResponseTemplate::new(200).set_body_json(entry_json(
                "new-dir",
                "/new-dir",
                "directory",
            )))
            .mount(&server)
            .await;

        let vol = vol_for(&server);
        let entry = vol
            .make_dir(
                "/new-dir",
                VolumeMakeDirOpts {
                    uid: Some(1000),
                    ..Default::default()
                },
            )
            .await
            .expect("make_dir ok");
        assert_eq!(entry.file_type, VolumeFileType::Directory);
        assert_eq!(entry.path, "/new-dir");
    }

    #[tokio::test]
    async fn stat_returns_entry() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumecontent/vol_1/path"))
            .and(query_param("path", "/a"))
            .and(header("Authorization", "Bearer tkn"))
            .respond_with(ResponseTemplate::new(200).set_body_json(entry_json("a", "/a", "file")))
            .mount(&server)
            .await;

        let vol = vol_for(&server);
        let entry = vol.stat("/a").await.expect("stat ok");
        assert_eq!(entry.file_type, VolumeFileType::File);
        assert_eq!(entry.path, "/a");
    }

    #[tokio::test]
    async fn update_metadata_patches() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/volumecontent/vol_1/path"))
            .and(query_param("path", "/a"))
            .and(body_partial_json(serde_json::json!({"uid": 1000})))
            .and(header("Authorization", "Bearer tkn"))
            .respond_with(ResponseTemplate::new(200).set_body_json(entry_json("a", "/a", "file")))
            .mount(&server)
            .await;

        let vol = vol_for(&server);
        let entry = vol
            .update_metadata(
                "/a",
                VolumeMetadataOpts {
                    uid: Some(1000),
                    ..Default::default()
                },
            )
            .await
            .expect("update_metadata ok");
        assert_eq!(entry.file_type, VolumeFileType::File);
    }

    #[tokio::test]
    async fn remove_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/volumecontent/vol_1/path"))
            .and(query_param("path", "/a"))
            .and(header("Authorization", "Bearer tkn"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let vol = vol_for(&server);
        vol.remove("/a").await.expect("remove ok");
    }

    #[tokio::test]
    async fn exists_true() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumecontent/vol_1/path"))
            .and(query_param("path", "/a"))
            .respond_with(ResponseTemplate::new(200).set_body_json(entry_json("a", "/a", "file")))
            .mount(&server)
            .await;

        let vol = vol_for(&server);
        assert!(vol.exists("/a").await.expect("exists ok"));
    }

    #[tokio::test]
    async fn exists_false() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumecontent/vol_1/path"))
            .and(query_param("path", "/missing"))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(
                    serde_json::json!({"code": "not_found", "message": "not found"}),
                ),
            )
            .mount(&server)
            .await;

        let vol = vol_for(&server);
        let result = vol
            .exists("/missing")
            .await
            .expect("exists returns Ok(false) for 404");
        assert!(!result);
    }

    #[tokio::test]
    async fn stat_404_path_message() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumecontent/vol_1/path"))
            .and(query_param("path", "/gone"))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(
                    serde_json::json!({"code": "not_found", "message": "not found"}),
                ),
            )
            .mount(&server)
            .await;

        let vol = vol_for(&server);
        let err = vol.stat("/gone").await.expect_err("stat should Err on 404");
        assert!(
            matches!(&err, Error::NotFound(msg) if msg.contains("/gone")),
            "expected NotFound with path in message, got: {err:?}",
        );
    }

    // ── File I/O tests (Task 4) ──────────────────────────────────────────────

    #[tokio::test]
    async fn read_file_text() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumecontent/vol_1/file"))
            .and(query_param("path", "/a.txt"))
            .and(header("Authorization", "Bearer tkn"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"hello" as &[u8])
                    .append_header("Content-Type", "application/octet-stream"),
            )
            .mount(&server)
            .await;

        let vol = vol_for(&server);
        let text = vol.read_file("/a.txt").await.expect("read_file ok");
        assert_eq!(text, "hello");
    }

    #[tokio::test]
    async fn read_file_bytes_returns_raw() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumecontent/vol_1/file"))
            .and(query_param("path", "/a.txt"))
            .and(header("Authorization", "Bearer tkn"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"hello" as &[u8])
                    .append_header("Content-Type", "application/octet-stream"),
            )
            .mount(&server)
            .await;

        let vol = vol_for(&server);
        let bytes = vol
            .read_file_bytes("/a.txt")
            .await
            .expect("read_file_bytes ok");
        assert_eq!(bytes, b"hello".to_vec());
    }

    #[tokio::test]
    async fn read_file_stream_collects() {
        use futures::StreamExt as _;

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumecontent/vol_1/file"))
            .and(query_param("path", "/a.txt"))
            .and(header("Authorization", "Bearer tkn"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"hello" as &[u8])
                    .append_header("Content-Type", "application/octet-stream"),
            )
            .mount(&server)
            .await;

        let vol = vol_for(&server);
        let stream = vol
            .read_file_stream("/a.txt", VolumeReadOpts::default())
            .await
            .expect("read_file_stream ok");
        let chunks: Vec<bytes::Bytes> = stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()
            .expect("collect stream chunks");
        let combined: Vec<u8> = chunks.into_iter().flat_map(|b| b.to_vec()).collect();
        assert_eq!(combined, b"hello");
    }

    #[tokio::test]
    async fn write_file_puts_octet_stream() {
        let server = MockServer::start().await;
        let response_json = entry_json("a.txt", "/a.txt", "file");
        Mock::given(method("PUT"))
            .and(path("/volumecontent/vol_1/file"))
            .and(query_param("path", "/a.txt"))
            .and(query_param("uid", "1000"))
            .and(header("Authorization", "Bearer tkn"))
            .and(header("Content-Type", "application/octet-stream"))
            .and(body_bytes(b"hello".to_vec()))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response_json))
            .mount(&server)
            .await;

        let vol = vol_for(&server);
        let entry = vol
            .write_file(
                "/a.txt",
                b"hello".to_vec(),
                VolumeWriteOpts {
                    uid: Some(1000),
                    ..Default::default()
                },
            )
            .await
            .expect("write_file ok");
        assert_eq!(entry.file_type, VolumeFileType::File);
        assert_eq!(entry.path, "/a.txt");
    }

    // ── Debug-redaction tests ────────────────────────────────────────────────

    #[test]
    fn volume_debug_redacts_token_and_proxy() {
        let vol = Volume::from_parts(
            "vol_redact1".to_string(),
            "redact-volume".to_string(),
            "supersecret-token".to_string(),
            "https://api.e2b.app".to_string(),
            60_000,
            Some("http://user:pass@proxy.example.com".to_string()),
        );
        let debug_str = format!("{vol:?}");
        assert!(
            !debug_str.contains("supersecret-token"),
            "token must not appear in Debug output: {debug_str}"
        );
        assert!(
            !debug_str.contains("user:pass"),
            "proxy credentials must not appear in Debug output: {debug_str}"
        );
        assert!(
            debug_str.contains("<redacted>"),
            "Debug output must contain '<redacted>': {debug_str}"
        );
        assert!(
            debug_str.contains("vol_redact1"),
            "volume_id must appear in Debug output: {debug_str}"
        );
        assert!(
            debug_str.contains("redact-volume"),
            "name must appear in Debug output: {debug_str}"
        );
    }

    #[test]
    fn volume_opts_debug_redacts_api_key_and_proxy() {
        let opts = VolumeOpts {
            api_key: Some("e2b_secret".to_string()),
            domain: Some("e2b.app".to_string()),
            api_url: Some("https://api.e2b.app".to_string()),
            request_timeout_ms: Some(30_000),
            proxy: Some("http://user:pass@proxy.example.com".to_string()),
        };
        let debug_str = format!("{opts:?}");
        assert!(
            !debug_str.contains("e2b_secret"),
            "api_key must not appear in Debug output: {debug_str}"
        );
        assert!(
            !debug_str.contains("user:pass"),
            "proxy credentials must not appear in Debug output: {debug_str}"
        );
        assert!(
            debug_str.contains("<redacted>"),
            "Debug output must contain '<redacted>': {debug_str}"
        );
        assert!(
            debug_str.contains("e2b.app"),
            "domain must appear in Debug output: {debug_str}"
        );
    }

    // ── 404-message parity tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn get_info_404_includes_volume_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumes/vol_missing"))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(
                    serde_json::json!({"code": "not_found", "message": "not found"}),
                ),
            )
            .mount(&server)
            .await;

        let err = Volume::get_info("vol_missing", opts_for(&server))
            .await
            .expect_err("get_info on 404 must Err");
        assert!(
            matches!(&err, Error::NotFound(msg) if msg.contains("vol_missing")),
            "expected NotFound with volume_id in message, got: {err:?}",
        );
    }

    #[tokio::test]
    async fn connect_404_includes_volume_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumes/vol_gone"))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(
                    serde_json::json!({"code": "not_found", "message": "not found"}),
                ),
            )
            .mount(&server)
            .await;

        let err = Volume::connect("vol_gone", opts_for(&server))
            .await
            .expect_err("connect on 404 must Err");
        assert!(
            matches!(&err, Error::NotFound(msg) if msg.contains("vol_gone")),
            "expected NotFound with volume_id in message, got: {err:?}",
        );
    }

    #[tokio::test]
    async fn remove_404_path_message() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/volumecontent/vol_1/path"))
            .and(query_param("path", "/gone"))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(
                    serde_json::json!({"code": "not_found", "message": "not found"}),
                ),
            )
            .mount(&server)
            .await;

        let vol = vol_for(&server);
        let err = vol
            .remove("/gone")
            .await
            .expect_err("remove should Err on 404");
        assert!(
            matches!(&err, Error::NotFound(msg) if msg.contains("/gone")),
            "expected NotFound with path in message, got: {err:?}",
        );
    }
}
