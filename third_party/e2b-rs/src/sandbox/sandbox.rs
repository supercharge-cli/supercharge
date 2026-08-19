//! The public `Sandbox` type and its control-plane lifecycle.

use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::time::Duration;

use crate::api::client::ApiClient;
use crate::connection_config::{ConnectionConfig, DEFAULT_USERNAME, ENVD_PORT};
use crate::envd::versions::version_gte;
use crate::errors::{Error, Result};
use crate::sandbox::api;
use crate::sandbox::opts::{SandboxConnectOpts, SandboxCreateOpts, SandboxUrlOpts};
use crate::sandbox::signature::{SignatureOperation, get_signature_now};
use crate::sandbox::types::{SandboxInfo, SandboxMetrics, SandboxState, SnapshotInfo};

/// A boxed future returned by the lifecycle builders.
type SandboxFuture = Pin<Box<dyn Future<Output = Result<Sandbox>> + Send>>;

/// A running or paused E2B sandbox.
///
/// Create one with [`Sandbox::create`] or reconnect with [`Sandbox::connect`].
///
/// # Example
/// ```no_run
/// # async fn run() -> e2b_rs::Result<()> {
/// use e2b_rs::Sandbox;
/// let sandbox = Sandbox::create().template("base").await?;
/// println!("sandbox {} at {}", sandbox.sandbox_id(), sandbox.get_host(3000));
/// sandbox.kill().await?;
/// # Ok(())
/// # }
/// ```
pub struct Sandbox {
    /// Sandbox identifier.
    pub(crate) sandbox_id: String,
    /// Optional per-sandbox domain override.
    pub(crate) sandbox_domain: Option<String>,
    /// envd version string; used by version gates and envd I/O in Plan 3b.
    pub(crate) envd_version: String,
    /// Access token for envd communication.
    pub(crate) envd_access_token: Option<String>,
    /// Resolved connection configuration.
    pub(crate) config: ConnectionConfig,
    /// Control-plane API client.
    pub(crate) api: ApiClient,
    /// Filesystem operations (`sandbox.files()`).
    pub(crate) files: crate::sandbox::filesystem::Filesystem,
    /// Command execution (`sandbox.commands()`).
    pub(crate) commands: crate::sandbox::commands::Commands,
    /// Pseudo-terminal sessions (`sandbox.pty()`).
    pub(crate) pty: crate::sandbox::pty::Pty,
    /// Git operations (`sandbox.git()`).
    pub(crate) git: crate::sandbox::git::Git,
}

impl Sandbox {
    /// Start configuring a new sandbox. Await the builder to create it.
    pub fn create() -> SandboxCreateBuilder {
        SandboxCreateBuilder {
            opts: SandboxCreateOpts::default(),
        }
    }

    /// Start configuring a reconnect to an existing (possibly paused) sandbox.
    pub fn connect(sandbox_id: impl Into<String>) -> SandboxConnectBuilder {
        SandboxConnectBuilder {
            sandbox_id: sandbox_id.into(),
            opts: SandboxConnectOpts::default(),
        }
    }

    /// The sandbox identifier.
    pub fn sandbox_id(&self) -> &str {
        &self.sandbox_id
    }

    /// The external host for a sandbox port, e.g. `3000-<id>.e2b.app`.
    pub fn get_host(&self, port: u16) -> String {
        let domain = self
            .sandbox_domain
            .as_deref()
            .unwrap_or(&self.config.domain);
        self.config.get_host(&self.sandbox_id, port, Some(domain))
    }

    /// Kill the sandbox. Returns `false` if it was already gone.
    pub async fn kill(&self) -> Result<bool> {
        api::kill_sandbox(&self.api, &self.sandbox_id).await
    }

    /// Fetch current sandbox info.
    pub async fn get_info(&self) -> Result<SandboxInfo> {
        let detail = api::get_sandbox_info(&self.api, &self.sandbox_id).await?;
        Ok(SandboxInfo::from_detail(detail))
    }

    /// Set the sandbox timeout, measured from now.
    pub async fn set_timeout(&self, timeout: Duration) -> Result<()> {
        api::set_sandbox_timeout(&self.api, &self.sandbox_id, timeout).await
    }

    /// Pause the sandbox, returning `false` if it was already paused.
    ///
    /// Takes a full memory snapshot so a later [`Sandbox::connect`] warm-boots.
    pub async fn pause(&self) -> Result<bool> {
        api::pause_sandbox(&self.api, &self.sandbox_id, true).await
    }

    /// Whether the sandbox is currently running (control-plane state).
    ///
    /// Note: Plan 3b refines this to the envd `/health` probe; for now it
    /// reflects the control-plane `state`.
    pub async fn is_running(&self) -> Result<bool> {
        Ok(matches!(
            self.get_info().await?.state,
            SandboxState::Running
        ))
    }

    /// Fetch the sandbox's resource-usage metrics.
    ///
    /// # Errors
    /// Returns [`Error::Template`] if the sandbox's envd is older than `0.1.5`
    /// (metrics are unsupported), matching the JS SDK.
    pub async fn get_metrics(&self) -> Result<Vec<SandboxMetrics>> {
        if !version_gte(&self.envd_version, "0.1.5") {
            return Err(Error::Template(
                "Metrics require a newer template (envd >= 0.1.5); rebuild the template."
                    .to_string(),
            ));
        }
        let raw = api::get_sandbox_metrics(&self.api, &self.sandbox_id, None, None).await?;
        Ok(raw.into_iter().map(SandboxMetrics::from_metric).collect())
    }

    /// Create a snapshot of this sandbox. `name` registers (or re-points) a
    /// template alias for the snapshot.
    pub async fn create_snapshot(&self, name: Option<String>) -> Result<SnapshotInfo> {
        let raw = api::create_snapshot(&self.api, &self.sandbox_id, name.as_deref()).await?;
        Ok(SnapshotInfo::from_schema(raw))
    }

    /// Delete a snapshot by id. Returns `false` if it was already gone.
    pub async fn delete_snapshot(
        snapshot_id: impl Into<String>,
        connection: crate::connection_config::ConnectionConfigOpts,
    ) -> Result<bool> {
        let config = ConnectionConfig::new(connection);
        let api = ApiClient::new(&config, true)?;
        api::delete_snapshot(&api, &snapshot_id.into()).await
    }

    /// List snapshots (paginated). Filter by source sandbox via
    /// [`crate::SnapshotListOpts::sandbox_id`].
    pub fn list_snapshots(
        opts: crate::sandbox::opts::SnapshotListOpts,
    ) -> Result<crate::sandbox::snapshot_paginator::SnapshotPaginator> {
        crate::sandbox::snapshot_paginator::SnapshotPaginator::new(opts)
    }

    /// List sandboxes (paginated). Filter by state/metadata via [`crate::SandboxListOpts`].
    ///
    /// Note: defaults to listing only `Running` and `Paused` sandboxes (the JS
    /// SDK lists all states by default); pass `SandboxListOpts::states` to override.
    pub fn list(
        opts: crate::sandbox::opts::SandboxListOpts,
    ) -> Result<crate::sandbox::paginator::SandboxPaginator> {
        crate::sandbox::paginator::SandboxPaginator::new(opts)
    }

    /// Atomically replace the sandbox's egress network policy.
    ///
    /// Note: this REPLACES the policy — fields left empty clear the
    /// corresponding server-side rules (no merge). A `409` here means the
    /// sandbox is paused (resume it first).
    pub async fn update_network(
        &self,
        update: crate::sandbox::network::SandboxNetworkUpdate,
    ) -> Result<()> {
        api::update_sandbox_network(&self.api, &self.sandbox_id, &update.to_wire_body()).await
    }

    /// Resolve the per-sandbox domain (override, else config default).
    fn resolved_domain(&self) -> &str {
        self.sandbox_domain
            .as_deref()
            .unwrap_or(&self.config.domain)
    }

    /// Build the base `/files` URL (`{envd_direct_url}/files?username&path`),
    /// percent-encoding the query values via `reqwest::Url`.
    fn file_url(&self, path: &str, user: Option<&str>) -> Result<reqwest::Url> {
        let base =
            self.config
                .get_sandbox_direct_url(&self.sandbox_id, self.resolved_domain(), ENVD_PORT);
        let mut url = reqwest::Url::parse(&format!("{base}/files"))
            .map_err(|e| Error::Internal(format!("invalid sandbox url: {e}")))?;
        {
            let mut q = url.query_pairs_mut();
            if let Some(user) = user {
                q.append_pair("username", user);
            }
            if !path.is_empty() {
                q.append_pair("path", path);
            }
        }
        Ok(url)
    }

    /// Build a signed (or unsigned) URL for `op` on `path`.
    fn signed_file_url(
        &self,
        path: &str,
        op: SignatureOperation,
        opts: &SandboxUrlOpts,
    ) -> Result<String> {
        let use_signature = self
            .envd_access_token
            .as_deref()
            .is_some_and(|t| !t.is_empty());
        if !use_signature && opts.signature_expiration_secs.is_some() {
            return Err(Error::InvalidArgument(
                "Signature expiration can be used only when the sandbox is created as secured."
                    .to_string(),
            ));
        }

        // Older envd (<0.4.0) has no per-request user; default to the legacy user.
        let user = match opts.user.as_deref() {
            Some(u) => Some(u.to_string()),
            None if !version_gte(&self.envd_version, "0.4.0") => Some(DEFAULT_USERNAME.to_string()),
            None => None,
        };

        let mut url = self.file_url(path, user.as_deref())?;
        if use_signature {
            let sig = get_signature_now(
                path,
                op,
                user.as_deref(),
                opts.signature_expiration_secs,
                self.envd_access_token.as_deref(),
            )?;
            url.query_pairs_mut()
                .append_pair("signature", &sig.signature);
            if let Some(exp) = sig.expiration {
                url.query_pairs_mut()
                    .append_pair("signature_expiration", &exp.to_string());
            }
        }
        Ok(url.to_string())
    }

    /// Build a URL for uploading a file to the sandbox (empty `path` = the
    /// default upload directory). Signed when the sandbox is secured.
    pub fn upload_url(&self, path: Option<&str>, opts: SandboxUrlOpts) -> Result<String> {
        self.signed_file_url(path.unwrap_or(""), SignatureOperation::Write, &opts)
    }

    /// Build a URL for downloading `path` from the sandbox. Signed when the
    /// sandbox is secured.
    pub fn download_url(&self, path: &str, opts: SandboxUrlOpts) -> Result<String> {
        self.signed_file_url(path, SignatureOperation::Read, &opts)
    }

    /// Build a `Sandbox` from a `create`/`connect` response (the lean
    /// `api::schema::Sandbox`) plus the resolved config/client.
    fn from_api_sandbox(
        s: crate::api::schema::Sandbox,
        config: ConnectionConfig,
        api: ApiClient,
    ) -> Result<Sandbox> {
        let sandbox_domain = s.domain.clone();
        let domain = sandbox_domain
            .clone()
            .unwrap_or_else(|| config.domain.clone());
        let files = crate::sandbox::filesystem::Filesystem::build(
            &s.sandbox_id,
            &domain,
            &s.envd_version.0,
            s.envd_access_token.as_deref(),
            &config,
        )?;
        let commands = crate::sandbox::commands::Commands::build(
            &s.sandbox_id,
            &domain,
            &s.envd_version.0,
            s.envd_access_token.as_deref(),
            &config,
        )?;
        // Clone `commands` before it is moved into the struct so `git` can
        // share the same underlying connect client.
        let git = crate::sandbox::git::Git::new(commands.clone());
        let pty = crate::sandbox::pty::Pty::build(
            &s.sandbox_id,
            &domain,
            &s.envd_version.0,
            s.envd_access_token.as_deref(),
            &config,
        )?;
        Ok(Sandbox {
            sandbox_id: s.sandbox_id,
            sandbox_domain,
            envd_version: s.envd_version.0,
            envd_access_token: s.envd_access_token,
            config,
            api,
            files,
            commands,
            pty,
            git,
        })
    }

    /// Access the sandbox filesystem (`read`/`write`/`list`/`watch`/...).
    pub fn files(&self) -> &crate::sandbox::filesystem::Filesystem {
        &self.files
    }

    /// Access sandbox command execution (`run`/`start`/...).
    pub fn commands(&self) -> &crate::sandbox::commands::Commands {
        &self.commands
    }

    /// Access sandbox pseudo-terminal sessions (`create`/`resize`/...).
    pub fn pty(&self) -> &crate::sandbox::pty::Pty {
        &self.pty
    }

    /// Access sandbox git operations (`init`/`commit`/`push`/...).
    pub fn git(&self) -> &crate::sandbox::git::Git {
        &self.git
    }
}

/// Builder for [`Sandbox::create`]; await it to create the sandbox.
pub struct SandboxCreateBuilder {
    opts: SandboxCreateOpts,
}

impl SandboxCreateBuilder {
    /// Template id or alias (default `"base"`).
    pub fn template(mut self, template: impl Into<String>) -> Self {
        self.opts.template = Some(template.into());
        self
    }

    /// Sandbox lifetime (default 5 minutes).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.opts.timeout = Some(timeout);
        self
    }

    /// Add metadata entries.
    pub fn metadata<K: Into<String>, V: Into<String>>(
        mut self,
        entries: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        self.opts
            .metadata
            .extend(entries.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    /// Add environment variables.
    pub fn envs<K: Into<String>, V: Into<String>>(
        mut self,
        entries: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        self.opts
            .envs
            .extend(entries.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    /// Set the API key.
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.opts.connection.api_key = Some(key.into());
        self
    }

    /// Set the domain.
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.opts.connection.domain = Some(domain.into());
        self
    }

    /// Override the API base URL (mainly for tests/self-hosting).
    pub fn api_url(mut self, url: impl Into<String>) -> Self {
        self.opts.connection.api_url = Some(url.into());
        self
    }

    /// Enable debug mode.
    pub fn debug(mut self, debug: bool) -> Self {
        self.opts.connection.debug = Some(debug);
        self
    }

    /// Whether to use a secure (authenticated) connection to the sandbox
    /// (defaults to `true`, matching the JS SDK).
    pub fn secure(mut self, secure: bool) -> Self {
        self.opts.secure = Some(secure);
        self
    }

    /// Whether the sandbox may access the public internet (defaults to `true`,
    /// matching the JS SDK).
    pub fn allow_internet_access(mut self, allow: bool) -> Self {
        self.opts.allow_internet_access = Some(allow);
        self
    }
}

impl IntoFuture for SandboxCreateBuilder {
    type Output = Result<Sandbox>;
    type IntoFuture = SandboxFuture;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let config = ConnectionConfig::new(self.opts.connection.clone());
            let api = ApiClient::new(&config, true)?;
            let sandbox = api::create_sandbox(&api, &self.opts).await?;
            Sandbox::from_api_sandbox(sandbox, config, api)
        })
    }
}

/// Builder for [`Sandbox::connect`]; await it to (re)connect.
pub struct SandboxConnectBuilder {
    sandbox_id: String,
    opts: SandboxConnectOpts,
}

impl SandboxConnectBuilder {
    /// Lifetime to set on (re)connect (default 5 minutes).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.opts.timeout = Some(timeout);
        self
    }

    /// Set the API key.
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.opts.connection.api_key = Some(key.into());
        self
    }

    /// Override the API base URL.
    pub fn api_url(mut self, url: impl Into<String>) -> Self {
        self.opts.connection.api_url = Some(url.into());
        self
    }

    /// Set the domain.
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.opts.connection.domain = Some(domain.into());
        self
    }
}

impl IntoFuture for SandboxConnectBuilder {
    type Output = Result<Sandbox>;
    type IntoFuture = SandboxFuture;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let config = ConnectionConfig::new(self.opts.connection.clone());
            let api = ApiClient::new(&config, true)?;
            let timeout = self.opts.timeout.unwrap_or(Duration::from_millis(
                crate::connection_config::DEFAULT_SANDBOX_TIMEOUT_MS,
            ));
            let sandbox = api::connect_sandbox(&api, &self.sandbox_id, timeout).await?;
            Sandbox::from_api_sandbox(sandbox, config, api)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn local_sandbox(token: Option<&str>) -> Sandbox {
        let config = crate::connection_config::ConnectionConfig::new(
            crate::connection_config::ConnectionConfigOpts {
                api_key: Some("e2b_0123456789abcdef".to_string()),
                domain: Some("e2b.app".to_string()),
                ..Default::default()
            },
        );
        let api = ApiClient::new(&config, true).expect("api");
        let files = crate::sandbox::filesystem::Filesystem::build(
            "sbx_u", "e2b.app", "0.6.0", token, &config,
        )
        .expect("files");
        let commands =
            crate::sandbox::commands::Commands::build("sbx_u", "e2b.app", "0.6.0", token, &config)
                .expect("commands");
        let git = crate::sandbox::git::Git::new(commands.clone());
        let pty = crate::sandbox::pty::Pty::build("sbx_u", "e2b.app", "0.6.0", token, &config)
            .expect("pty");
        Sandbox {
            sandbox_id: "sbx_u".to_string(),
            sandbox_domain: Some("e2b.app".to_string()),
            envd_version: "0.6.0".to_string(),
            envd_access_token: token.map(str::to_string),
            config,
            api,
            files,
            commands,
            pty,
            git,
        }
    }

    #[test]
    fn download_url_without_token_is_unsigned() {
        let sandbox = local_sandbox(None);
        let url = sandbox
            .download_url("/home/user/f.txt", Default::default())
            .expect("url");
        assert!(url.contains("/files"));
        assert!(
            url.contains("path=%2Fhome%2Fuser%2Ff.txt") || url.contains("path=/home/user/f.txt")
        );
        assert!(!url.contains("signature="));
    }

    #[test]
    fn download_url_with_token_is_signed() {
        let sandbox = local_sandbox(Some("tok_abc"));
        let url = sandbox
            .download_url("/f.txt", Default::default())
            .expect("url");
        assert!(url.contains("signature=v1_"));
    }

    #[test]
    fn expiration_without_token_is_an_error() {
        let sandbox = local_sandbox(None);
        let err = sandbox
            .upload_url(
                Some("/f.txt"),
                crate::sandbox::opts::SandboxUrlOpts {
                    signature_expiration_secs: Some(60),
                    ..Default::default()
                },
            )
            .expect_err("must reject expiration without token");
        assert!(matches!(err, Error::InvalidArgument(_)));
    }

    /// The rich `SandboxDetail` returned by `GET /sandboxes/{id}`.
    fn detail_json(id: &str, state: &str) -> serde_json::Value {
        serde_json::json!({
            "sandboxID": id, "templateID": "base", "clientID": "c1",
            "cpuCount": 2, "memoryMB": 1024, "diskSizeMB": 1024,
            "envdVersion": "0.6.0", "state": state, "domain": "e2b.app",
            "startedAt": "2026-06-30T10:00:00Z", "endAt": "2026-06-30T10:05:00Z"
        })
    }

    /// The lean `Sandbox` returned by `POST /sandboxes` and `/connect` — no
    /// `cpuCount`/`memoryMB`/`state` fields, matching the real wire response.
    fn sandbox_json(id: &str) -> serde_json::Value {
        serde_json::json!({
            "sandboxID": id, "templateID": "base", "clientID": "c1",
            "envdVersion": "0.6.0", "domain": "e2b.app"
        })
    }

    #[tokio::test]
    async fn create_builds_a_sandbox_and_awaits_directly() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sandboxes"))
            .respond_with(ResponseTemplate::new(201).set_body_json(sandbox_json("sbx_c")))
            .mount(&server)
            .await;
        // Builder + direct `.await` (IntoFuture).
        let sandbox = Sandbox::create()
            .template("base")
            .timeout(Duration::from_secs(60))
            .api_key("e2b_0123456789abcdef")
            .domain("e2b.app")
            .api_url(server.uri())
            .await
            .expect("create");
        assert_eq!(sandbox.sandbox_id(), "sbx_c");
        assert_eq!(sandbox.get_host(3000), "3000-sbx_c.e2b.app");
    }

    #[tokio::test]
    async fn get_info_and_kill_round_trip() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sandboxes"))
            .respond_with(ResponseTemplate::new(201).set_body_json(sandbox_json("sbx_i")))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/sandboxes/sbx_i"))
            .respond_with(ResponseTemplate::new(200).set_body_json(detail_json("sbx_i", "running")))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/sandboxes/sbx_i"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let sandbox = Sandbox::create()
            .api_key("e2b_0123456789abcdef")
            .api_url(server.uri())
            .await
            .expect("create");
        let info = sandbox.get_info().await.expect("info");
        assert_eq!(info.sandbox_id, "sbx_i");
        assert!(sandbox.is_running().await.expect("running")); // state == running
        assert!(sandbox.kill().await.expect("kill"));
    }
}
