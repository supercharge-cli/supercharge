//! Filesystem API for sandbox file operations.

pub(crate) mod io;
pub mod types;
pub(crate) mod watch;

pub use io::FsWriteOpts;
pub use types::{EntryInfo, FileType, FilesystemEvent, FilesystemEventType, WriteEntry, WriteInfo};
pub use watch::{WatchHandle, WatchOpts};

use crate::connect::client::{ConnectClient, ConnectClientOpts};
use crate::connection_config::{ConnectionConfig, DEFAULT_USERNAME, ENVD_PORT};
use crate::envd::proto::filesystem as pb;
use crate::envd::rest::{EnvdApiClient, EnvdApiClientOpts};
use crate::envd::versions::{ENVD_DEFAULT_USER, version_gte};
use crate::errors::{Error, Result};

/// Map a filesystem `NotFound` to the file-specific [`Error::FileNotFound`]
/// (matching the JS SDK, which raises `FileNotFoundError`).
pub(crate) fn file_not_found_on_missing(err: Error, path: &str) -> Error {
    match err {
        Error::NotFound(_) => Error::FileNotFound(format!("File not found: {path}")),
        other => other,
    }
}

/// The sandbox filesystem: read/write byte I/O over the envd REST `/files`
/// surface plus metadata operations over the Connect `Filesystem` service.
pub struct Filesystem {
    /// Connect-over-JSON client for the envd RPC surface.
    pub(crate) connect: ConnectClient,
    /// REST client for the envd `/files` surface (used by Tasks 3–5).
    pub(crate) rest: EnvdApiClient,
    /// envd version string; used by version gates (used by Tasks 3–6).
    pub(crate) envd_version: String,
    /// Legacy default user (set on old envd < 0.4.0, `None` on modern envd).
    pub(crate) default_user: Option<String>,
}

impl Filesystem {
    /// Build a `Filesystem` for a sandbox, resolving the envd base URL from the
    /// connection config (envd port `49983`).
    pub(crate) fn build(
        sandbox_id: &str,
        sandbox_domain: &str,
        envd_version: &str,
        envd_access_token: Option<&str>,
        config: &ConnectionConfig,
    ) -> Result<Filesystem> {
        let base_url = config.get_sandbox_url(sandbox_id, sandbox_domain, ENVD_PORT);
        Self::build_with_base_url(
            base_url,
            sandbox_id,
            envd_version,
            envd_access_token,
            config,
        )
    }

    /// Build directly from a base URL (used by tests + by [`Filesystem::build`]).
    pub(crate) fn build_with_base_url(
        base_url: String,
        sandbox_id: &str,
        envd_version: &str,
        envd_access_token: Option<&str>,
        config: &ConnectionConfig,
    ) -> Result<Filesystem> {
        let user_agent = crate::utils::build_user_agent(config.integration.as_deref());
        let connect = ConnectClient::new(ConnectClientOpts {
            base_url: base_url.clone(),
            access_token: envd_access_token.map(str::to_string),
            sandbox_id: sandbox_id.to_string(),
            envd_port: ENVD_PORT,
            user_agent: user_agent.clone(),
            envd_version: envd_version.to_string(),
            request_timeout_ms: config.request_timeout_ms,
            logger: config.logger.clone(),
            proxy: config.proxy.clone(),
        })?;
        let rest = EnvdApiClient::new(EnvdApiClientOpts {
            base_url,
            access_token: envd_access_token.map(str::to_string),
            sandbox_id: sandbox_id.to_string(),
            envd_port: ENVD_PORT,
            user_agent,
            request_timeout_ms: config.request_timeout_ms,
            logger: config.logger.clone(),
            proxy: config.proxy.clone(),
        })?;
        // Older envd (<0.4.0) has no per-request user; default to the legacy user.
        let default_user =
            (!version_gte(envd_version, ENVD_DEFAULT_USER)).then(|| DEFAULT_USERNAME.to_string());
        Ok(Filesystem {
            connect,
            rest,
            envd_version: envd_version.to_string(),
            default_user,
        })
    }

    /// Resolve the user for a request: explicit `user`, else the legacy default
    /// on old envd, else `None`.
    pub(crate) fn resolve_user(&self, user: Option<&str>) -> Option<String> {
        match user {
            Some(u) => Some(u.to_string()),
            None => self.default_user.clone(),
        }
    }

    /// Get metadata for a path. Errors with [`Error::FileNotFound`] if missing.
    pub async fn get_info(&self, path: &str, user: Option<&str>) -> Result<EntryInfo> {
        let user = self.resolve_user(user);
        let req = pb::StatRequest {
            path: path.to_string(),
        };
        let resp: pb::StatResponse = self
            .connect
            .unary(crate::connect::FS_STAT, &req, user.as_deref())
            .await
            .map_err(|e| file_not_found_on_missing(e, path))?;
        resp.entry
            .and_then(EntryInfo::from_proto)
            .ok_or_else(|| Error::Internal(format!("Stat returned no entry for {path}")))
    }

    /// Whether a path exists.
    pub async fn exists(&self, path: &str, user: Option<&str>) -> Result<bool> {
        match self.get_info(path, user).await {
            Ok(_) => Ok(true),
            Err(Error::FileNotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// List directory entries, descending `depth` levels (default 1). Entries
    /// with an unknown file type are skipped.
    pub async fn list(
        &self,
        path: &str,
        depth: Option<u32>,
        user: Option<&str>,
    ) -> Result<Vec<EntryInfo>> {
        let depth = depth.unwrap_or(1);
        if depth < 1 {
            return Err(Error::InvalidArgument(
                "list depth must be at least 1".to_string(),
            ));
        }
        let user = self.resolve_user(user);
        let req = pb::ListDirRequest {
            path: path.to_string(),
            depth,
        };
        let resp: pb::ListDirResponse = self
            .connect
            .unary(crate::connect::FS_LIST_DIR, &req, user.as_deref())
            .await
            .map_err(|e| file_not_found_on_missing(e, path))?;
        Ok(resp
            .entries
            .into_iter()
            .filter_map(EntryInfo::from_proto)
            .collect())
    }

    /// Create a directory. Returns `false` if it already exists.
    pub async fn make_dir(&self, path: &str, user: Option<&str>) -> Result<bool> {
        let user = self.resolve_user(user);
        let req = pb::MakeDirRequest {
            path: path.to_string(),
        };
        match self
            .connect
            .unary::<_, pb::MakeDirResponse>(crate::connect::FS_MAKE_DIR, &req, user.as_deref())
            .await
        {
            Ok(_) => Ok(true),
            Err(Error::Conflict(_)) => Ok(false),
            Err(e) => Err(file_not_found_on_missing(e, path)),
        }
    }

    /// Remove a file or directory.
    pub async fn remove(&self, path: &str, user: Option<&str>) -> Result<()> {
        let user = self.resolve_user(user);
        let req = pb::RemoveRequest {
            path: path.to_string(),
        };
        self.connect
            .unary::<_, pb::RemoveResponse>(crate::connect::FS_REMOVE, &req, user.as_deref())
            .await
            .map(|_| ())
            .map_err(|e| file_not_found_on_missing(e, path))
    }

    /// Move/rename an entry, returning the moved entry's info.
    pub async fn rename(
        &self,
        old_path: &str,
        new_path: &str,
        user: Option<&str>,
    ) -> Result<EntryInfo> {
        let user = self.resolve_user(user);
        let req = pb::MoveRequest {
            source: old_path.to_string(),
            destination: new_path.to_string(),
        };
        let resp: pb::MoveResponse = self
            .connect
            .unary(crate::connect::FS_MOVE, &req, user.as_deref())
            .await
            .map_err(|e| file_not_found_on_missing(e, old_path))?;
        resp.entry
            .and_then(EntryInfo::from_proto)
            .ok_or_else(|| Error::Internal(format!("Move returned no entry for {new_path}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection_config::{ConnectionConfig, ConnectionConfigOpts};
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn read_text_and_bytes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/files"))
            .and(wiremock::matchers::query_param("path", "/f.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
            .mount(&server)
            .await;
        let fs = fs_for(&server);
        assert_eq!(fs.read("/f.txt", None).await.expect("text"), "hello");
        assert_eq!(
            fs.read_bytes("/f.txt", None).await.expect("bytes"),
            b"hello"
        );
    }

    #[tokio::test]
    async fn read_missing_is_file_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/files"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let err = fs_for(&server).read("/nope", None).await.expect_err("404");
        assert!(matches!(err, crate::errors::Error::FileNotFound(_)));
    }

    fn fs_for(server: &MockServer) -> Filesystem {
        let config = ConnectionConfig::new(ConnectionConfigOpts {
            api_key: Some("e2b_0123456789abcdef".to_string()),
            ..Default::default()
        });
        // Point the envd clients straight at the mock server.
        Filesystem::build_with_base_url(server.uri(), "sbx_fs", "0.6.3", None, &config)
            .expect("filesystem")
    }

    fn entry_json(path: &str) -> serde_json::Value {
        serde_json::json!({
            "name": "f.txt", "type": "FILE_TYPE_FILE", "path": path,
            "size": "12", "mode": 420, "permissions": "-rw-r--r--",
            "owner": "user", "group": "user", "metadata": {}
        })
    }

    #[tokio::test]
    async fn get_info_returns_entry() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/filesystem.Filesystem/Stat"))
            .and(body_partial_json(
                serde_json::json!({ "path": "/home/user/f.txt" }),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "entry": entry_json("/home/user/f.txt") })),
            )
            .mount(&server)
            .await;
        let info = fs_for(&server)
            .get_info("/home/user/f.txt", None)
            .await
            .expect("info");
        assert_eq!(info.path, "/home/user/f.txt");
        assert_eq!(info.r#type, FileType::File);
    }

    #[tokio::test]
    async fn exists_false_on_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/filesystem.Filesystem/Stat"))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(
                    serde_json::json!({ "code": "not_found", "message": "missing" }),
                ),
            )
            .mount(&server)
            .await;
        assert!(!fs_for(&server).exists("/nope", None).await.expect("exists"));
    }

    #[tokio::test]
    async fn list_returns_entries_and_filters_unknown() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/filesystem.Filesystem/ListDir"))
            .and(body_partial_json(
                serde_json::json!({ "path": "/d", "depth": 1 }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "entries": [
                    entry_json("/d/f.txt"),
                    { "name": "weird", "type": "FILE_TYPE_UNSPECIFIED", "path": "/d/weird" }
                ]
            })))
            .mount(&server)
            .await;
        let out = fs_for(&server).list("/d", None, None).await.expect("list");
        assert_eq!(out.len(), 1); // unknown-type entry filtered out
        assert_eq!(out[0].path, "/d/f.txt");
    }

    #[tokio::test]
    async fn list_rejects_zero_depth() {
        let server = MockServer::start().await;
        let err = fs_for(&server)
            .list("/d", Some(0), None)
            .await
            .expect_err("depth");
        assert!(matches!(err, crate::errors::Error::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn make_dir_false_on_already_exists() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/filesystem.Filesystem/MakeDir"))
            .respond_with(ResponseTemplate::new(409).set_body_json(
                serde_json::json!({ "code": "already_exists", "message": "exists" }),
            ))
            .mount(&server)
            .await;
        assert!(!fs_for(&server).make_dir("/d", None).await.expect("makedir"));
    }

    #[tokio::test]
    async fn write_single_octet_stream_returns_info() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/files"))
            .and(wiremock::matchers::query_param("path", "/w.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                // REST /files response: lowercase `type` (NOT the proto FILE_TYPE_*).
                { "name": "w.txt", "type": "file", "path": "/w.txt", "metadata": {} }
            ])))
            .mount(&server)
            .await;
        let fs = fs_for(&server);
        let opts = crate::sandbox::filesystem::FsWriteOpts {
            use_octet_stream: Some(true),
            ..Default::default()
        };
        let info = fs
            .write("/w.txt", b"hi".to_vec(), opts)
            .await
            .expect("write");
        assert_eq!(info.path, "/w.txt");
        // The REST `"file"` type must decode to the public FileType::File.
        assert_eq!(info.r#type, Some(FileType::File));
    }

    #[tokio::test]
    async fn write_gzip_on_old_envd_falls_back_to_multipart() {
        // JS silently downgrades octet/gzip to multipart on envd < 0.5.7 (no error).
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "name": "w.txt", "type": "file", "path": "/w.txt", "metadata": {} }
            ])))
            .mount(&server)
            .await;
        let config = ConnectionConfig::new(ConnectionConfigOpts::default());
        let fs = Filesystem::build_with_base_url(server.uri(), "sbx", "0.5.0", None, &config)
            .expect("fs");
        let opts = crate::sandbox::filesystem::FsWriteOpts {
            gzip: true,
            ..Default::default()
        };
        // Must NOT error with Error::Template — falls back to multipart.
        let info = fs
            .write("/w.txt", b"hi".to_vec(), opts)
            .await
            .expect("write");
        assert_eq!(info.path, "/w.txt");
    }

    #[tokio::test]
    async fn write_rejects_metadata_on_old_envd() {
        // fs_for uses envd 0.6.3 >= 0.6.2, so build an old-envd fs explicitly.
        let server = MockServer::start().await;
        let config = ConnectionConfig::new(ConnectionConfigOpts::default());
        let fs = Filesystem::build_with_base_url(server.uri(), "sbx", "0.6.1", None, &config)
            .expect("fs");
        let mut opts = crate::sandbox::filesystem::FsWriteOpts::default();
        opts.metadata.insert("k".into(), "v".into());
        let err = fs
            .write("/w.txt", b"hi".to_vec(), opts)
            .await
            .expect_err("gate");
        assert!(matches!(err, crate::errors::Error::Template(_)));
    }

    #[tokio::test]
    async fn write_files_ignores_gzip_no_octet_gate_on_old_envd() {
        // write_files is always multipart; a stray gzip flag must NOT trip the
        // octet-stream version gate (it would on `write`).
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "name": "a.txt", "type": "file", "path": "/a.txt", "metadata": {} }
            ])))
            .mount(&server)
            .await;
        let config = ConnectionConfig::new(ConnectionConfigOpts::default());
        // 0.5.0 < 0.5.7 octet floor — a gated `write(gzip)` would error here.
        let fs = Filesystem::build_with_base_url(server.uri(), "sbx", "0.5.0", None, &config)
            .expect("fs");
        let opts = crate::sandbox::filesystem::FsWriteOpts {
            gzip: true,
            ..Default::default()
        };
        let entries = vec![crate::sandbox::filesystem::WriteEntry {
            path: "/a.txt".to_string(),
            data: b"hi".to_vec(),
        }];
        let out = fs.write_files(entries, opts).await.expect("write_files");
        assert_eq!(out.len(), 1);
    }

    #[tokio::test]
    async fn rename_sends_source_then_destination() {
        let server = MockServer::start().await;
        // Locks the source/destination ordering (old_path -> source, new_path -> destination).
        Mock::given(method("POST"))
            .and(path("/filesystem.Filesystem/Move"))
            .and(body_partial_json(
                serde_json::json!({ "source": "/a.txt", "destination": "/b.txt" }),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "entry": entry_json("/b.txt") })),
            )
            .mount(&server)
            .await;
        let info = fs_for(&server)
            .rename("/a.txt", "/b.txt", None)
            .await
            .expect("rename");
        assert_eq!(info.path, "/b.txt");
    }
}
