//! The `CommandHandle` and the stream-driving infrastructure for Commands/Pty.

use std::sync::Arc;

use futures::StreamExt as _;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::types::{CommandOutput, CommandResult};
use crate::connect::client::ConnectClient;
use crate::envd::proto::process as pb;
use crate::envd::proto::process::process_event::Event as ProcEvent;
// NOTE: `DataEvent` (and its `Output` oneof) are nested UNDER `process_event`
// by prost, so the path is `process_event::data_event::Output` (NOT top-level).
use crate::envd::proto::process::process_event::data_event::Output as DataOutput;
use crate::envd::versions::{ENVD_ENVD_CLOSE, version_gte};
use crate::errors::{Error, Result};

/// Build a `ProcessSelector` selecting a process by pid.
pub(crate) fn pid_selector(pid: u32) -> pb::ProcessSelector {
    pb::ProcessSelector {
        selector: Some(pb::process_selector::Selector::Pid(pid)),
    }
}

/// A running command (or pty). Receive live output with [`CommandHandle::next`]
/// and the final result with [`CommandHandle::wait`].
pub struct CommandHandle {
    pid: u32,
    output: mpsc::Receiver<CommandOutput>,
    /// Carries the final outcome: `Ok(CommandResult)` on a clean finish (any
    /// exit code), or `Err` if the process stream failed mid-flight (transport
    /// error). Wrapped in `Option` so `wait` can `.take()` it without a
    /// partial-move out of a type that implements `Drop`.
    result: Option<oneshot::Receiver<Result<CommandResult>>>,
    task: JoinHandle<()>,
    connect: Arc<ConnectClient>,
    envd_version: String,
    user: Option<String>,
}

impl CommandHandle {
    /// The process id.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Receive the next chunk of live output, or `None` when the process has
    /// produced all output (it may still be finishing — call [`Self::wait`]).
    pub async fn next(&mut self) -> Option<CommandOutput> {
        self.output.recv().await
    }

    /// Wait for the command to finish and return its [`CommandResult`]. Drains
    /// any unread output first (so the background task can complete). A non-zero
    /// `exit_code` is returned in the result, NOT as an error; an `Err` means the
    /// process stream itself failed (transport/RPC error) before completing.
    pub async fn wait(mut self) -> Result<CommandResult> {
        while self.output.recv().await.is_some() {}
        // `.take()` uses `&mut self.result` (no partial move), which is
        // permitted even though `Self: Drop`.
        let rx = self
            .result
            .take()
            .ok_or_else(|| Error::Internal("wait called twice".to_string()))?;
        match rx.await {
            Ok(outcome) => outcome,
            Err(_) => Err(Error::Internal(
                "command ended without a result".to_string(),
            )),
        }
    }

    /// Kill the process (SIGKILL). Returns `false` if it was not found.
    pub async fn kill(&self) -> Result<bool> {
        let req = pb::SendSignalRequest {
            process: Some(pid_selector(self.pid)),
            signal: pb::Signal::Sigkill as i32,
        };
        match self
            .connect
            .unary::<_, pb::SendSignalResponse>(
                crate::connect::PROC_SEND_SIGNAL,
                &req,
                self.user.as_deref(),
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(Error::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Write bytes to the process's stdin.
    pub async fn send_stdin(&self, data: &[u8]) -> Result<()> {
        let req = pb::SendInputRequest {
            process: Some(pid_selector(self.pid)),
            input: Some(pb::ProcessInput {
                input: Some(pb::process_input::Input::Stdin(data.to_vec())),
            }),
        };
        self.connect
            .unary::<_, pb::SendInputResponse>(
                crate::connect::PROC_SEND_INPUT,
                &req,
                self.user.as_deref(),
            )
            .await
            .map(|_| ())
    }

    /// Close the process's stdin (requires a newer envd).
    pub async fn close_stdin(&self) -> Result<()> {
        if !version_gte(&self.envd_version, ENVD_ENVD_CLOSE) {
            return Err(Error::Template(format!(
                "close_stdin requires a newer template (envd >= {ENVD_ENVD_CLOSE})"
            )));
        }
        let req = pb::CloseStdinRequest {
            process: Some(pid_selector(self.pid)),
        };
        self.connect
            .unary::<_, pb::CloseStdinResponse>(
                crate::connect::PROC_CLOSE_STDIN,
                &req,
                self.user.as_deref(),
            )
            .await
            .map(|_| ())
    }
}

impl Drop for CommandHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Extract the `Event` from a `StartResponse`/`ConnectResponse`-shaped message
/// (both wrap `ProcessEvent`).
#[allow(dead_code)] // used by the spawned task inside open_handle (via closure capture)
fn response_event(resp: pb::StartResponse) -> Option<ProcEvent> {
    resp.event.and_then(|e| e.event)
}

/// Open a process server-stream (`Start`/`Connect`), peek the first `StartEvent`
/// for the pid, then spawn a task forwarding output into the handle's channel.
///
/// `req` must be `'static` because the stream opened by `server_stream` is
/// spawned into a `tokio::task` (which requires `'static` bounds).
#[allow(dead_code)] // used by Task 2+ (Commands::run, connect, pty)
pub(crate) async fn open_handle(
    connect: Arc<ConnectClient>,
    path: &str,
    req: &(impl Serialize + 'static),
    user: Option<&str>,
    envd_version: &str,
) -> Result<CommandHandle> {
    // `Box::pin` so the stream is `Unpin` and can move into the spawned task.
    let mut stream = Box::pin(
        connect
            .server_stream::<_, pb::StartResponse>(path, req, user)
            .await?,
    );

    // Wait for the StartEvent (skip any leading KeepAlive).
    let pid = loop {
        match stream.next().await {
            Some(Ok(resp)) => match response_event(resp) {
                Some(ProcEvent::Start(s)) => break s.pid,
                Some(ProcEvent::Keepalive(_)) | None => continue,
                Some(_) => {
                    return Err(Error::Internal(
                        "process stream sent output before the start event".to_string(),
                    ));
                }
            },
            Some(Err(e)) => return Err(e),
            None => {
                return Err(Error::Internal(
                    "process stream ended before the start event".to_string(),
                ));
            }
        }
    };

    let (output_tx, output_rx) = mpsc::channel(128);
    let (result_tx, result_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let mut exit_code: i32 = 0;
        let mut error: Option<String> = None;
        let mut got_end = false;
        let mut stream_error: Option<Error> = None;
        while let Some(item) = stream.next().await {
            let resp = match item {
                Ok(resp) => resp,
                // A transport/RPC error mid-stream: capture it, surface as Err.
                Err(e) => {
                    stream_error = Some(e);
                    break;
                }
            };
            match response_event(resp) {
                // `let _ =` on send: if the receiver was dropped we keep
                // accumulating output for `wait()` (only the live channel closes).
                Some(ProcEvent::Data(d)) => match d.output {
                    Some(DataOutput::Stdout(b)) => {
                        stdout.extend_from_slice(&b);
                        let _ = output_tx.send(CommandOutput::Stdout(b)).await;
                    }
                    Some(DataOutput::Stderr(b)) => {
                        stderr.extend_from_slice(&b);
                        let _ = output_tx.send(CommandOutput::Stderr(b)).await;
                    }
                    Some(DataOutput::Pty(b)) => {
                        let _ = output_tx.send(CommandOutput::Pty(b)).await;
                    }
                    None => {}
                },
                Some(ProcEvent::End(end)) => {
                    exit_code = end.exit_code;
                    error = end.error;
                    got_end = true;
                    break;
                }
                // Start (already consumed) + KeepAlive are not surfaced.
                _ => {}
            }
        }
        let outcome = if let Some(e) = stream_error {
            // A genuine transport/RPC failure mid-stream propagates as `Err`.
            Err(e)
        } else {
            // Clean close without an EndEvent: report it in the result rather
            // than as a clean exit_code 0.
            if !got_end && error.is_none() {
                error = Some("process stream closed before the end event".to_string());
            }
            Ok(CommandResult {
                exit_code,
                error,
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            })
        };
        let _ = result_tx.send(outcome);
    });

    Ok(CommandHandle {
        pid,
        output: output_rx,
        result: Some(result_rx),
        task,
        connect,
        envd_version: envd_version.to_string(),
        user: user.map(str::to_string),
    })
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

    #[tokio::test]
    async fn handle_streams_stdout_then_exit() {
        let server = MockServer::start().await;
        // pbjson proto3-JSON: ProcessEvent oneof = field-name key; DataEvent stdout = base64.
        let mut body = encode_envelope(0, br#"{"event":{"start":{"pid":42}}}"#);
        body.extend(encode_envelope(
            0,
            br#"{"event":{"data":{"stdout":"aGk="}}}"#,
        ));
        body.extend(encode_envelope(
            0,
            br#"{"event":{"end":{"exitCode":0,"exited":true,"status":"exited"}}}"#,
        ));
        body.extend(encode_envelope(FLAG_END_STREAM, b"{}"));
        Mock::given(method("POST"))
            .and(path("/process.Process/Start"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+json")
                    .set_body_bytes(body),
            )
            .mount(&server)
            .await;

        let connect = connect_for(&server);
        let req = crate::envd::proto::process::StartRequest {
            process: Some(crate::envd::proto::process::ProcessConfig {
                cmd: "/bin/bash".to_string(),
                args: vec!["-l".to_string(), "-c".to_string(), "echo hi".to_string()],
                envs: Default::default(),
                cwd: None,
            }),
            pty: None,
            tag: None,
            stdin: Some(false),
        };
        let mut handle = open_handle(connect, crate::connect::PROC_START, &req, None, "0.6.3")
            .await
            .expect("handle");
        assert_eq!(handle.pid(), 42);
        match handle.next().await {
            Some(CommandOutput::Stdout(b)) => assert_eq!(b, b"hi"),
            other => panic!("expected stdout, got {other:?}"),
        }
        assert!(handle.next().await.is_none()); // stream ended
        let result = handle.wait().await.expect("result");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hi");
    }

    #[tokio::test]
    async fn stream_closing_before_end_event_is_reported() {
        // Start event, then the stream closes with NO end event — wait() must
        // surface an error rather than reporting a clean exit_code 0.
        let server = MockServer::start().await;
        let mut body = encode_envelope(0, br#"{"event":{"start":{"pid":1}}}"#);
        body.extend(encode_envelope(FLAG_END_STREAM, b"{}"));
        Mock::given(method("POST"))
            .and(path("/process.Process/Start"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+json")
                    .set_body_bytes(body),
            )
            .mount(&server)
            .await;
        let req = crate::envd::proto::process::StartRequest {
            process: Some(crate::envd::proto::process::ProcessConfig {
                cmd: "/bin/bash".to_string(),
                args: vec!["-l".to_string(), "-c".to_string(), "x".to_string()],
                envs: Default::default(),
                cwd: None,
            }),
            pty: None,
            tag: None,
            stdin: Some(false),
        };
        let handle = open_handle(
            connect_for(&server),
            crate::connect::PROC_START,
            &req,
            None,
            "0.6.3",
        )
        .await
        .expect("handle");
        let result = handle.wait().await.expect("result");
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn mid_stream_transport_error_propagates_as_err() {
        // Start event, then an end-stream ERROR frame (a transport/RPC failure):
        // wait() must return Err, not Ok with a generic message.
        let server = MockServer::start().await;
        let mut body = encode_envelope(0, br#"{"event":{"start":{"pid":3}}}"#);
        body.extend(encode_envelope(
            FLAG_END_STREAM,
            br#"{"error":{"code":"not_found","message":"gone"}}"#,
        ));
        Mock::given(method("POST"))
            .and(path("/process.Process/Start"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+json")
                    .set_body_bytes(body),
            )
            .mount(&server)
            .await;
        let req = crate::envd::proto::process::StartRequest {
            process: Some(crate::envd::proto::process::ProcessConfig {
                cmd: "/bin/bash".to_string(),
                args: vec!["-l".to_string(), "-c".to_string(), "x".to_string()],
                envs: Default::default(),
                cwd: None,
            }),
            pty: None,
            tag: None,
            stdin: Some(false),
        };
        let handle = open_handle(
            connect_for(&server),
            crate::connect::PROC_START,
            &req,
            None,
            "0.6.3",
        )
        .await
        .expect("handle");
        let err = handle.wait().await.expect_err("transport error");
        assert!(matches!(err, Error::NotFound(_)));
    }
}
