//! Spawn Supercharge `agent stdio` and speak ACP. The harness stays in that process.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_client_protocol::{self as acp, Agent, Client, ClientSideConnection, Error};
use anyhow::Context;
use futures::io::BufReader;
use supercharge_tty_utils::ProcessScope;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::protocol::{
    self, TranscriptLine, format_session_update, initialize_request, new_session_request,
    prompt_request,
};

pub enum UiEvent {
    Status(String),
    Line(TranscriptLine),
    Permission(PendingPermission),
    Ready { session_id: String },
    TurnDone,
    Error(String),
}

pub struct PendingPermission {
    pub title: String,
    pub options: Vec<(String, String)>,
    pub reply: oneshot::Sender<acp::RequestPermissionResponse>,
}

pub enum AppCommand {
    Prompt(String),
    CancelTurn,
}

struct DesktopClient {
    ui: std::sync::mpsc::Sender<UiEvent>,
}

fn acp_io_error(err: impl ToString) -> Error {
    Error::internal_error().data(serde_json::Value::String(err.to_string()))
}

#[async_trait::async_trait(?Send)]
impl Client for DesktopClient {
    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> Result<acp::RequestPermissionResponse, Error> {
        let title = args
            .tool_call
            .fields
            .title
            .clone()
            .unwrap_or_else(|| "Tool permission".to_string());
        let options = args
            .options
            .iter()
            .map(|opt| (opt.option_id.0.to_string(), opt.name.clone()))
            .collect();
        let (tx, rx) = oneshot::channel();
        let _ = self.ui.send(UiEvent::Permission(PendingPermission {
            title,
            options,
            reply: tx,
        }));
        Ok(rx.await.unwrap_or_else(|_| {
            acp::RequestPermissionResponse::new(acp::RequestPermissionOutcome::Cancelled)
        }))
    }

    async fn session_notification(&self, args: acp::SessionNotification) -> Result<(), Error> {
        if let Some(line) = format_session_update(&args.update) {
            let _ = self.ui.send(UiEvent::Line(line));
        }
        Ok(())
    }

    async fn write_text_file(
        &self,
        args: acp::WriteTextFileRequest,
    ) -> Result<acp::WriteTextFileResponse, Error> {
        if let Some(parent) = args.path.parent() {
            std::fs::create_dir_all(parent).map_err(acp_io_error)?;
        }
        std::fs::write(&args.path, args.content).map_err(acp_io_error)?;
        Ok(acp::WriteTextFileResponse::new())
    }

    async fn read_text_file(
        &self,
        args: acp::ReadTextFileRequest,
    ) -> Result<acp::ReadTextFileResponse, Error> {
        let raw = std::fs::read_to_string(&args.path).map_err(acp_io_error)?;
        let mut lines: Vec<&str> = raw.lines().collect();
        if let Some(start) = args.line {
            let idx = start.saturating_sub(1) as usize;
            if idx < lines.len() {
                lines = lines.split_off(idx);
            } else {
                lines.clear();
            }
        }
        if let Some(limit) = args.limit {
            lines.truncate(limit as usize);
        }
        Ok(acp::ReadTextFileResponse::new(lines.join("\n")))
    }
}

pub fn spawn_runtime(
    agent_bin: PathBuf,
    cwd: PathBuf,
    ui: std::sync::mpsc::Sender<UiEvent>,
) -> std::sync::mpsc::Sender<AppCommand> {
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<AppCommand>();
    std::thread::Builder::new()
        .name("supercharge-acp".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            let local = tokio::task::LocalSet::new();
            // Bridge the std channel into the tokio loop via a helper thread.
            let (tx_bridge, rx_bridge) = tokio::sync::mpsc::unbounded_channel::<AppCommand>();
            std::thread::Builder::new()
                .name("supercharge-acp-bridge".into())
                .spawn(move || {
                    while let Ok(cmd) = cmd_rx.recv() {
                        if tx_bridge.send(cmd).is_err() {
                            break;
                        }
                    }
                })
                .expect("spawn bridge thread");
            local.block_on(&rt, run_agent(agent_bin, cwd, ui, rx_bridge));
        })
        .expect("spawn ACP thread");
    cmd_tx
}

async fn run_agent(
    agent_bin: PathBuf,
    cwd: PathBuf,
    ui: std::sync::mpsc::Sender<UiEvent>,
    mut cmd_rx: mpsc::UnboundedReceiver<AppCommand>,
) {
    let _ = ui.send(UiEvent::Status(format!(
        "starting {} agent stdio in {}",
        agent_bin.display(),
        cwd.display()
    )));

    let mut spawned = match spawn_agent(&agent_bin) {
        Ok(spawned) => spawned,
        Err(err) => {
            let _ = ui.send(UiEvent::Error(format!("{err:#}")));
            return;
        }
    };
    let stdin = spawned.child.stdin.take().expect("agent stdin");
    let stdout = spawned.child.stdout.take().expect("agent stdout");

    let io_ui = ui.clone();
    let (conn, io_task) = ClientSideConnection::new(
        DesktopClient { ui: ui.clone() },
        stdin.compat_write(),
        BufReader::new(stdout.compat()),
        |fut| {
            tokio::task::spawn_local(fut);
        },
    );
    tokio::task::spawn_local(async move {
        if let Err(err) = io_task.await {
            let _ = io_ui.send(UiEvent::Error(format!("ACP IO ended: {err}")));
        }
    });

    let child = Arc::new(Mutex::new(Some(spawned)));
    let child_for_drop = child.clone();

    if let Err(err) = handshake(&conn, &ui).await {
        let _ = ui.send(UiEvent::Error(format!("{err:#}")));
        kill_child(&child_for_drop);
        return;
    }

    let session_id = match conn.new_session(new_session_request(cwd.clone())).await {
        Ok(resp) => resp.session_id,
        Err(err) => {
            let _ = ui.send(UiEvent::Error(format!(
                "session/new failed: {err}. If auth is required, run `supercharge login` first."
            )));
            kill_child(&child_for_drop);
            return;
        }
    };
    let _ = ui.send(UiEvent::Ready {
        session_id: session_id.0.to_string(),
    });

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            AppCommand::Prompt(text) => {
                let _ = ui.send(UiEvent::Line(TranscriptLine {
                    kind: protocol::LineKind::User,
                    text: text.clone(),
                    ..TranscriptLine::default()
                }));
                match conn.prompt(prompt_request(session_id.clone(), text)).await {
                    Ok(_) => {
                        let _ = ui.send(UiEvent::TurnDone);
                    }
                    Err(err) => {
                        let _ = ui.send(UiEvent::Error(format!("session/prompt failed: {err}")));
                    }
                }
            }
            AppCommand::CancelTurn => {
                if let Err(err) = conn
                    .cancel(acp::CancelNotification::new(session_id.clone()))
                    .await
                {
                    let _ = ui.send(UiEvent::Error(format!("session/cancel failed: {err}")));
                }
            }
        }
    }
    kill_child(&child_for_drop);
}

async fn handshake(
    conn: &ClientSideConnection,
    ui: &std::sync::mpsc::Sender<UiEvent>,
) -> anyhow::Result<()> {
    let resp = conn
        .initialize(initialize_request())
        .await
        .context("initialize")?;
    let _ = ui.send(UiEvent::Status(format!(
        "ACP initialized (protocol {:?})",
        resp.protocol_version
    )));
    Ok(())
}

struct SpawnedAgent {
    child: Child,
    /// Strong enrollment handle; dropping it makes `ProcessScope::kill_all` skip this PID.
    _enrolled: Box<dyn Send>,
    scope: ProcessScope,
}

fn spawn_agent(bin: &PathBuf) -> anyhow::Result<SpawnedAgent> {
    let scope = ProcessScope::new();
    let mut cmd = Command::new(bin);
    cmd.args(["agent", "stdio"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true);
    let (child, group) = scope
        .spawn(cmd)
        .with_context(|| format!("failed to spawn {} agent stdio", bin.display()))?;
    Ok(SpawnedAgent {
        child,
        _enrolled: Box::new(group),
        scope,
    })
}

fn kill_child(child: &Arc<Mutex<Option<SpawnedAgent>>>) {
    if let Ok(mut guard) = child.lock()
        && let Some(mut spawned) = guard.take()
    {
        let _ = spawned.child.start_kill();
        spawned.scope.kill_all();
    }
}
