//! Sandbox commands and PTY APIs (Process service).
//!
//! This module provides the `Commands` struct for running and managing sandbox
//! processes, as well as the reusable handle infrastructure.

pub(crate) mod handle;
pub mod types;

pub use handle::CommandHandle;
pub use types::{CommandOutput, CommandResult, ProcessInfo};

#[allow(unused_imports)] // used by Task 2+ callers (run, connect, pty)
pub(crate) use handle::{open_handle, pid_selector};

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::connect::client::{ConnectClient, ConnectClientOpts};
use crate::connection_config::{ConnectionConfig, DEFAULT_USERNAME, ENVD_PORT};
use crate::envd::proto::process as pb;
use crate::envd::versions::{ENVD_COMMANDS_STDIN, ENVD_DEFAULT_USER, version_gte};
use crate::errors::{Error, Result};

/// Build a [`ConnectClient`] for a sandbox's envd RPC surface (shared by
/// [`Commands`] and the pty module).
pub(crate) fn build_connect_client(
    sandbox_id: &str,
    sandbox_domain: &str,
    envd_version: &str,
    envd_access_token: Option<&str>,
    config: &ConnectionConfig,
) -> Result<ConnectClient> {
    let base_url = config.get_sandbox_url(sandbox_id, sandbox_domain, ENVD_PORT);
    ConnectClient::new(ConnectClientOpts {
        base_url,
        access_token: envd_access_token.map(str::to_string),
        sandbox_id: sandbox_id.to_string(),
        envd_port: ENVD_PORT,
        user_agent: crate::utils::build_user_agent(config.integration.as_deref()),
        envd_version: envd_version.to_string(),
        request_timeout_ms: config.request_timeout_ms,
        logger: config.logger.clone(),
        proxy: config.proxy.clone(),
    })
}

/// Options for [`Commands::run`] / [`Commands::start`].
#[derive(Default)]
pub struct CommandStartOpts {
    /// Working directory for the command.
    pub cwd: Option<String>,
    /// User to run the command as (overrides the sandbox default).
    pub user: Option<String>,
    /// Additional environment variables for the command.
    pub envs: BTreeMap<String, String>,
    /// Whether stdin is enabled. Defaults to `false`; setting `Some(false)`
    /// on older envd (< 0.3.0) returns an error.
    pub stdin: Option<bool>,
}

/// Run and manage commands in the sandbox.
///
/// Obtain via [`crate::Sandbox::commands`].
#[derive(Clone)]
pub struct Commands {
    /// Connect-over-JSON client for the envd RPC surface.
    pub(crate) connect: Arc<ConnectClient>,
    /// envd version string; used by version gates.
    pub(crate) envd_version: String,
    /// Legacy default user (set on old envd < 0.4.0, `None` on modern envd).
    pub(crate) default_user: Option<String>,
}

impl Commands {
    /// Build a `Commands` for a sandbox, resolving the envd base URL from the
    /// connection config.
    pub(crate) fn build(
        sandbox_id: &str,
        sandbox_domain: &str,
        envd_version: &str,
        envd_access_token: Option<&str>,
        config: &ConnectionConfig,
    ) -> Result<Commands> {
        let connect = build_connect_client(
            sandbox_id,
            sandbox_domain,
            envd_version,
            envd_access_token,
            config,
        )?;
        Ok(Self::build_with_connect(Arc::new(connect), envd_version))
    }

    /// Build directly from an existing [`ConnectClient`] (used by tests and the
    /// pty module).
    #[allow(dead_code)] // used by tests and Task 5 (Pty)
    pub(crate) fn build_with_connect(connect: Arc<ConnectClient>, envd_version: &str) -> Commands {
        let default_user =
            (!version_gte(envd_version, ENVD_DEFAULT_USER)).then(|| DEFAULT_USERNAME.to_string());
        Commands {
            connect,
            envd_version: envd_version.to_string(),
            default_user,
        }
    }

    /// Resolve the effective user for a request: explicit `user`, else the
    /// legacy default on old envd, else `None`.
    #[allow(dead_code)] // used by tests and later tasks
    pub(crate) fn resolve_user(&self, user: Option<&str>) -> Option<String> {
        match user {
            Some(u) => Some(u.to_string()),
            None => self.default_user.clone(),
        }
    }

    fn start_request(&self, cmd: &str, opts: &CommandStartOpts) -> Result<pb::StartRequest> {
        if opts.stdin == Some(false) && !version_gte(&self.envd_version, ENVD_COMMANDS_STDIN) {
            return Err(Error::Template(format!(
                "this template's envd cannot disable stdin (requires >= {ENVD_COMMANDS_STDIN}); rebuild the template"
            )));
        }
        Ok(pb::StartRequest {
            process: Some(pb::ProcessConfig {
                cmd: "/bin/bash".to_string(),
                args: vec!["-l".to_string(), "-c".to_string(), cmd.to_string()],
                envs: opts.envs.clone().into_iter().collect(),
                cwd: opts.cwd.clone(),
            }),
            pty: None,
            tag: None,
            stdin: Some(opts.stdin.unwrap_or(false)),
        })
    }

    /// Start a command in the background, returning a [`CommandHandle`] for live
    /// output and control.
    pub async fn start(&self, cmd: &str, opts: CommandStartOpts) -> Result<CommandHandle> {
        let user = self.resolve_user(opts.user.as_deref());
        let req = self.start_request(cmd, &opts)?;
        open_handle(
            Arc::clone(&self.connect),
            crate::connect::PROC_START,
            &req,
            user.as_deref(),
            &self.envd_version,
        )
        .await
    }

    /// Run a command to completion and return its [`CommandResult`]. A non-zero
    /// `exit_code` is returned in the result, NOT as an error.
    pub async fn run(&self, cmd: &str, opts: CommandStartOpts) -> Result<CommandResult> {
        self.start(cmd, opts).await?.wait().await
    }

    /// List running processes.
    pub async fn list(&self) -> Result<Vec<ProcessInfo>> {
        let user = self.default_user.clone();
        let resp: pb::ListResponse = self
            .connect
            .unary(
                crate::connect::PROC_LIST,
                &pb::ListRequest {},
                user.as_deref(),
            )
            .await?;
        Ok(resp
            .processes
            .into_iter()
            .map(ProcessInfo::from_proto)
            .collect())
    }

    /// Kill a process by pid (SIGKILL). Returns `false` if it was not found.
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

    /// Write bytes to a process's stdin.
    pub async fn send_stdin(&self, pid: u32, data: &[u8]) -> Result<()> {
        let user = self.default_user.clone();
        let req = pb::SendInputRequest {
            process: Some(pid_selector(pid)),
            input: Some(pb::ProcessInput {
                input: Some(pb::process_input::Input::Stdin(data.to_vec())),
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

    /// Reconnect to a running process by pid, returning a [`CommandHandle`].
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

    /// Close a process's stdin (requires envd >= `ENVD_ENVD_CLOSE`).
    pub async fn close_stdin(&self, pid: u32) -> Result<()> {
        if !version_gte(&self.envd_version, crate::envd::versions::ENVD_ENVD_CLOSE) {
            return Err(Error::Template(format!(
                "close_stdin requires a newer template (envd >= {})",
                crate::envd::versions::ENVD_ENVD_CLOSE
            )));
        }
        let user = self.default_user.clone();
        let req = pb::CloseStdinRequest {
            process: Some(pid_selector(pid)),
        };
        self.connect
            .unary::<_, pb::CloseStdinResponse>(
                crate::connect::PROC_CLOSE_STDIN,
                &req,
                user.as_deref(),
            )
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::client::{ConnectClient, ConnectClientOpts};
    use crate::connect::envelope::{FLAG_END_STREAM, encode_envelope};
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

    fn build_with_connect(connect: Arc<ConnectClient>, envd_version: &str) -> Commands {
        Commands::build_with_connect(connect, envd_version)
    }

    #[tokio::test]
    async fn run_foreground_returns_result() {
        let server = MockServer::start().await;
        let mut body = encode_envelope(0, br#"{"event":{"start":{"pid":7}}}"#);
        body.extend(encode_envelope(
            0,
            br#"{"event":{"data":{"stdout":"b3V0"}}}"#,
        )); // "out"
        body.extend(encode_envelope(
            0,
            br#"{"event":{"end":{"exitCode":3,"exited":true,"status":"exited"}}}"#,
        ));
        body.extend(encode_envelope(FLAG_END_STREAM, b"{}"));
        // NOTE: body_partial_json cannot match enveloped streaming bodies (the
        // 5-byte binary envelope header makes the body non-JSON). Match only on
        // method + path; the assertions below verify the correct command was built.
        Mock::given(method("POST"))
            .and(path("/process.Process/Start"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+json")
                    .set_body_bytes(body),
            )
            .mount(&server)
            .await;
        let commands = build_with_connect(connect_for(&server), "0.6.3");
        let result = commands
            .run("echo out", CommandStartOpts::default())
            .await
            .expect("run");
        // Non-zero exit is data, not an error.
        assert_eq!(result.exit_code, 3);
        assert_eq!(result.stdout, "out");
    }

    #[tokio::test]
    async fn list_maps_processes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/process.Process/List"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "processes": [
                    { "pid": 11, "config": { "cmd": "/bin/bash", "args": ["-l","-c","sleep 1"], "envs": {} } }
                ]
            })))
            .mount(&server)
            .await;
        let out = build_with_connect(connect_for(&server), "0.6.3")
            .list()
            .await
            .expect("list");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pid, 11);
        assert_eq!(out[0].cmd, "/bin/bash");
    }

    #[tokio::test]
    async fn connect_reattaches_to_process() {
        let server = MockServer::start().await;
        let mut body = encode_envelope(0, br#"{"event":{"start":{"pid":5}}}"#);
        body.extend(encode_envelope(
            0,
            br#"{"event":{"data":{"stdout":"aGk="}}}"#,
        ));
        body.extend(encode_envelope(
            0,
            br#"{"event":{"end":{"exitCode":0,"exited":true,"status":"exited"}}}"#,
        ));
        body.extend(encode_envelope(FLAG_END_STREAM, b"{}"));
        // NOTE: body_partial_json cannot match enveloped streaming bodies (the
        // 5-byte binary envelope header makes the body non-JSON). Match only on
        // method + path; the pid and output assertions fully verify the contract.
        Mock::given(method("POST"))
            .and(path("/process.Process/Connect"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+json")
                    .set_body_bytes(body),
            )
            .mount(&server)
            .await;
        let mut handle = build_with_connect(connect_for(&server), "0.6.3")
            .connect(5, None)
            .await
            .expect("connect");
        assert_eq!(handle.pid(), 5);
        assert!(matches!(
            handle.next().await,
            Some(CommandOutput::Stdout(_))
        ));
    }

    #[tokio::test]
    async fn kill_false_on_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/process.Process/SendSignal"))
            .respond_with(ResponseTemplate::new(404).set_body_json(
                serde_json::json!({ "code": "not_found", "message": "no such process" }),
            ))
            .mount(&server)
            .await;
        assert!(
            !build_with_connect(connect_for(&server), "0.6.3")
                .kill(99)
                .await
                .expect("kill")
        );
    }
}
