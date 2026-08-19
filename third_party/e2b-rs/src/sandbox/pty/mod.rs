//! Pseudo-terminal sessions in the sandbox (`sandbox.pty()`).

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::connect::client::ConnectClient;
use crate::connection_config::{ConnectionConfig, DEFAULT_USERNAME};
use crate::envd::proto::process as pb;
use crate::envd::versions::{ENVD_DEFAULT_USER, version_gte};
use crate::errors::{Error, Result};
use crate::sandbox::commands::{CommandHandle, build_connect_client, open_handle, pid_selector};

/// Terminal dimensions.
#[derive(Debug, Clone, Copy)]
pub struct PtySize {
    /// Number of columns.
    pub cols: u32,
    /// Number of rows.
    pub rows: u32,
}

/// Options for [`Pty::create`].
#[derive(Default)]
pub struct PtyCreateOpts {
    /// User to run the shell as.
    pub user: Option<String>,
    /// Extra environment variables (merged with defaults; caller values win).
    pub envs: BTreeMap<String, String>,
    /// Working directory for the shell.
    pub cwd: Option<String>,
}

/// Manage pseudo-terminal sessions (`sandbox.pty()`).
///
/// Obtain via [`crate::Sandbox::pty`].
pub struct Pty {
    /// Connect-over-JSON client for the envd RPC surface.
    pub(crate) connect: Arc<ConnectClient>,
    /// envd version string; used by version gates.
    pub(crate) envd_version: String,
    /// Legacy default user (set on old envd < 0.4.0, `None` on modern envd).
    pub(crate) default_user: Option<String>,
}

impl Pty {
    /// Build a `Pty` for a sandbox, resolving the envd base URL from the
    /// connection config.
    pub(crate) fn build(
        sandbox_id: &str,
        sandbox_domain: &str,
        envd_version: &str,
        envd_access_token: Option<&str>,
        config: &ConnectionConfig,
    ) -> Result<Pty> {
        let connect = build_connect_client(
            sandbox_id,
            sandbox_domain,
            envd_version,
            envd_access_token,
            config,
        )?;
        let default_user =
            (!version_gte(envd_version, ENVD_DEFAULT_USER)).then(|| DEFAULT_USERNAME.to_string());
        Ok(Pty {
            connect: Arc::new(connect),
            envd_version: envd_version.to_string(),
            default_user,
        })
    }

    /// Build directly from an existing [`ConnectClient`] (used by tests).
    #[allow(dead_code)]
    pub(crate) fn build_with_connect(connect: Arc<ConnectClient>, envd_version: &str) -> Pty {
        let default_user =
            (!version_gte(envd_version, ENVD_DEFAULT_USER)).then(|| DEFAULT_USERNAME.to_string());
        Pty {
            connect,
            envd_version: envd_version.to_string(),
            default_user,
        }
    }

    /// Resolve the effective user for a request.
    fn resolve_user(&self, user: Option<&str>) -> Option<String> {
        match user {
            Some(u) => Some(u.to_string()),
            None => self.default_user.clone(),
        }
    }

    /// Start an interactive pty session (`/bin/bash -i -l`). Output arrives as
    /// [`crate::CommandOutput::Pty`] on the returned handle.
    pub async fn create(&self, size: PtySize, opts: PtyCreateOpts) -> Result<CommandHandle> {
        let user = self.resolve_user(opts.user.as_deref());
        let mut envs: BTreeMap<String, String> = opts.envs.clone();
        envs.entry("TERM".to_string())
            .or_insert_with(|| "xterm-256color".to_string());
        envs.entry("LANG".to_string())
            .or_insert_with(|| "C.UTF-8".to_string());
        envs.entry("LC_ALL".to_string())
            .or_insert_with(|| "C.UTF-8".to_string());
        let req = pb::StartRequest {
            process: Some(pb::ProcessConfig {
                cmd: "/bin/bash".to_string(),
                args: vec!["-i".to_string(), "-l".to_string()],
                envs: envs.into_iter().collect(),
                cwd: opts.cwd.clone(),
            }),
            pty: Some(pb::Pty {
                size: Some(pb::pty::Size {
                    cols: size.cols,
                    rows: size.rows,
                }),
            }),
            tag: None,
            // JS omits `stdin` for a pty (its I/O flows through the pty master),
            // so leave it unset for 1:1 wire parity.
            stdin: None,
        };
        open_handle(
            Arc::clone(&self.connect),
            crate::connect::PROC_START,
            &req,
            user.as_deref(),
            &self.envd_version,
        )
        .await
    }

    /// Send raw bytes to the pty.
    pub async fn send_input(&self, pid: u32, data: &[u8]) -> Result<()> {
        let user = self.default_user.clone();
        let req = pb::SendInputRequest {
            process: Some(pid_selector(pid)),
            input: Some(pb::ProcessInput {
                input: Some(pb::process_input::Input::Pty(data.to_vec())),
            }),
        };
        self.connect
            .unary::<_, pb::SendInputResponse>(
                crate::connect::PROC_SEND_INPUT,
                &req,
                user.as_deref(),
            )
            .await
            .map(|_| ())
    }

    /// Resize the pty.
    pub async fn resize(&self, pid: u32, size: PtySize) -> Result<()> {
        let user = self.default_user.clone();
        let req = pb::UpdateRequest {
            process: Some(pid_selector(pid)),
            pty: Some(pb::Pty {
                size: Some(pb::pty::Size {
                    cols: size.cols,
                    rows: size.rows,
                }),
            }),
        };
        self.connect
            .unary::<_, pb::UpdateResponse>(crate::connect::PROC_UPDATE, &req, user.as_deref())
            .await
            .map(|_| ())
    }

    /// Kill the pty's process (SIGKILL). Returns `false` if not found.
    pub async fn kill(&self, pid: u32) -> Result<bool> {
        let user = self.default_user.clone();
        let req = pb::SendSignalRequest {
            process: Some(pid_selector(pid)),
            signal: pb::Signal::Sigkill as i32,
        };
        match self
            .connect
            .unary::<_, pb::SendSignalResponse>(
                crate::connect::PROC_SEND_SIGNAL,
                &req,
                user.as_deref(),
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(Error::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Reconnect to a running pty by pid.
    pub async fn connect(&self, pid: u32, user: Option<&str>) -> Result<CommandHandle> {
        let user = self.resolve_user(user);
        let req = pb::ConnectRequest {
            process: Some(pid_selector(pid)),
        };
        open_handle(
            Arc::clone(&self.connect),
            crate::connect::PROC_CONNECT,
            &req,
            user.as_deref(),
            &self.envd_version,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::client::{ConnectClient, ConnectClientOpts};
    use crate::connect::envelope::{FLAG_END_STREAM, encode_envelope};
    use crate::sandbox::commands::CommandOutput;
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn connect_for(server: &MockServer) -> Arc<ConnectClient> {
        Arc::new(
            ConnectClient::new(ConnectClientOpts {
                base_url: server.uri(),
                access_token: None,
                sandbox_id: "sbx_c".to_string(),
                envd_port: 49983,
                user_agent: "e2b-rs-test".to_string(),
                envd_version: "0.6.3".to_string(),
                request_timeout_ms: 60_000,
                logger: None,
                proxy: None,
            })
            .expect("connect client"),
        )
    }

    fn build_pty(connect: Arc<ConnectClient>) -> Pty {
        Pty::build_with_connect(connect, "0.6.3")
    }

    #[tokio::test]
    async fn create_streams_pty_output() {
        let server = MockServer::start().await;
        // Build a streaming response: StartEvent(pid=9) + Data(pty) + End + end-stream.
        // "aGVsbG8=" is base64 for "hello"
        let mut body = encode_envelope(0, br#"{"event":{"start":{"pid":9}}}"#);
        body.extend(encode_envelope(
            0,
            br#"{"event":{"data":{"pty":"aGVsbG8="}}}"#,
        ));
        body.extend(encode_envelope(
            0,
            br#"{"event":{"end":{"exitCode":0,"exited":true,"status":"exited"}}}"#,
        ));
        body.extend(encode_envelope(FLAG_END_STREAM, b"{}"));

        // NOTE: body_partial_json CANNOT match enveloped streaming bodies (the
        // 5-byte binary envelope header makes the body non-JSON). Match only on
        // method + path.
        Mock::given(method("POST"))
            .and(path("/process.Process/Start"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+json")
                    .set_body_bytes(body),
            )
            .mount(&server)
            .await;

        let pty = build_pty(connect_for(&server));
        let mut handle = pty
            .create(PtySize { cols: 80, rows: 24 }, PtyCreateOpts::default())
            .await
            .expect("create");

        assert_eq!(handle.pid(), 9);
        match handle.next().await {
            Some(CommandOutput::Pty(b)) => assert_eq!(b, b"hello"),
            other => panic!("expected CommandOutput::Pty, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resize_sends_update_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/process.Process/Update"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "process": { "pid": 9 },
                "pty": { "size": { "cols": 100, "rows": 40 } }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        let pty = build_pty(connect_for(&server));
        pty.resize(
            9,
            PtySize {
                cols: 100,
                rows: 40,
            },
        )
        .await
        .expect("resize");
    }
}
