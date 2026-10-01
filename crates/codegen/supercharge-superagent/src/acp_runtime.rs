use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{Notify, mpsc, oneshot},
};

use crate::redaction::redact_secrets;

type Response = Result<Value, String>;
type EventHandler = Arc<dyn Fn(Value) + Send + Sync>;

struct PendingRequest {
    response: oneshot::Sender<Response>,
    prompt: bool,
}

struct RuntimeState {
    pending: Mutex<HashMap<u64, PendingRequest>>,
    alive: AtomicBool,
    prompting: AtomicBool,
    settled: Notify,
    sequence: AtomicU64,
    on_event: EventHandler,
}

impl RuntimeState {
    fn fail(&self, error: String) {
        if !self.alive.swap(false, Ordering::SeqCst) {
            return;
        }
        self.prompting.store(false, Ordering::SeqCst);
        for (_, pending) in self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
        {
            let _ = pending.response.send(Err(error.clone()));
        }
        self.settled.notify_waiters();
        (self.on_event)(json!({"type":"runtime_exit", "error":error}));
    }
}

pub struct PersistentCli {
    state: Arc<RuntimeState>,
    writer: mpsc::UnboundedSender<Value>,
    shutdown: mpsc::UnboundedSender<()>,
    pub session_id: String,
    pub pid: u32,
    home: PathBuf,
    mcp_servers: Mutex<Vec<Value>>,
}

pub struct CliLaunch {
    pub binary: PathBuf,
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub model: String,
    pub session_id: String,
    pub resume: bool,
    pub rules: String,
    pub bot_environment: Option<BotToolEnvironment>,
    pub mcp_servers: Vec<Value>,
    pub workload_uid: Option<u32>,
    pub workload_gid: Option<u32>,
}

pub struct BotToolEnvironment {
    pub instance_id: String,
    pub bot_id: String,
    pub site: String,
    pub capability: String,
    pub chat_id: String,
}

struct ProcessGroup(Option<u32>);

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0 {
            // The daemon owns this dedicated process group, including background tools.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

impl PersistentCli {
    pub async fn launch(config: CliLaunch, on_event: EventHandler) -> Result<Self, String> {
        let mut command = tokio::process::Command::new(&config.binary);
        command
            .args([
                "--no-auto-update",
                "agent",
                "--no-leader",
                "--always-approve",
                "--model",
                &config.model,
                "stdio",
            ])
            .env("SUPERCHARGE_HOME", &config.home)
            .env("SUPERCHARGE_RUNTIME_MODE", "e2b")
            .env("SUPERCHARGE_AGENT", "supercharge-e2b")
            .env("SUPERCHARGE_DISABLE_AUTOUPDATER", "1")
            .env("SUPERCHARGE_MAX_CONCURRENT_SUBAGENTS", "2")
            .env("SUPERCHARGE_SUBAGENTS_MAX_DEPTH", "1")
            .current_dir(&config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for key in [
            "VECTRA_BOT_ID",
            "VECTRA_BOT_INSTANCE_ID",
            "VECTRA_BOT_SITE",
            "VECTRA_BOT_CAPABILITY",
            "VECTRA_BOT_SESSION_ID",
            "VECTRA_BOT_CHAT_ID",
        ] {
            command.env_remove(key);
        }
        crate::browser::configure_desktop_environment(&mut command, &config.session_id).await;
        if let Some(bot) = &config.bot_environment {
            command
                .env("VECTRA_BOT_ID", &bot.bot_id)
                .env("VECTRA_BOT_SESSION_ID", &config.session_id);
            if !bot.capability.is_empty() {
                command
                    .env("VECTRA_BOT_INSTANCE_ID", &bot.instance_id)
                    .env("VECTRA_BOT_CHAT_ID", &bot.chat_id)
                    .env("VECTRA_BOT_SITE", &bot.site)
                    .env("VECTRA_BOT_CAPABILITY", &bot.capability);
            }
        }
        for key in [
            "E2B_API_KEY",
            "SUPERAGENT_API_TOKEN",
            "SUPERAGENT_GATEWAY_TOKEN",
            "SUPERAGENT_APP_INBOX_SECRET",
            "XIBECODE_GATEWAY_TOKEN",
            "XIBECODE_APP_INBOX_SECRET",
        ] {
            command.env_remove(key);
        }
        #[cfg(unix)]
        command.process_group(0);
        match (config.workload_uid, config.workload_gid) {
            (Some(uid), Some(gid)) => {
                crate::workload_identity::apply_tokio_as(&mut command, uid, gid)
            }
            _ => crate::workload_identity::apply_tokio(&mut command),
        }
        let mut retries = 0;
        let mut child = loop {
            match command.spawn() {
                Ok(child) => break child,
                Err(error) if error.raw_os_error() == Some(26) && retries < 3 => {
                    retries += 1;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => return Err(redact_secrets(&error.to_string())),
            }
        };
        let pid = child.id().ok_or("Agent process has no PID")?;
        let mut stdin = child.stdin.take().ok_or("Agent stdin is missing")?;
        let stdout = child.stdout.take().ok_or("Agent stdout is missing")?;
        let stderr = child.stderr.take().ok_or("Agent stderr is missing")?;
        let state = Arc::new(RuntimeState {
            pending: Mutex::new(HashMap::new()),
            alive: AtomicBool::new(true),
            prompting: AtomicBool::new(false),
            settled: Notify::new(),
            sequence: AtomicU64::new(1),
            on_event,
        });
        let (writer, mut writes) = mpsc::unbounded_channel::<Value>();
        let (shutdown, mut shutdown_rx) = mpsc::unbounded_channel();
        let writing = tokio::spawn(async move {
            while let Some(message) = writes.recv().await {
                let mut bytes = serde_json::to_vec(&message).map_err(|e| e.to_string())?;
                bytes.push(b'\n');
                stdin.write_all(&bytes).await.map_err(|e| e.to_string())?;
                stdin.flush().await.map_err(|e| e.to_string())?;
            }
            Ok::<(), String>(())
        });
        let output_state = state.clone();
        let replies = writer.clone();
        let reading = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if let Some(method) = message.get("method").and_then(Value::as_str) {
                    if let Some(id) = message.get("id") {
                        // Unattended permissions are set at launch; unexpected interactive requests fail closed.
                        let response = if method == "session/request_permission" {
                            json!({"jsonrpc":"2.0", "id":id, "result":{"outcome":{"outcome":"cancelled"}}})
                        } else {
                            json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32601,"message":"Unsupported client method"}})
                        };
                        let _ = replies.send(response);
                    } else {
                        (output_state.on_event)(message);
                    }
                } else if let Some(id) = message.get("id").and_then(Value::as_u64) {
                    let pending = output_state
                        .pending
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&id);
                    if let Some(pending) = pending {
                        if pending.prompt {
                            output_state.prompting.store(false, Ordering::SeqCst);
                            output_state.settled.notify_waiters();
                        }
                        let result = if let Some(error) = message.get("error") {
                            Err(redact_secrets(&error.to_string()))
                        } else {
                            Ok(message.get("result").cloned().unwrap_or(Value::Null))
                        };
                        let _ = pending.response.send(result);
                    }
                }
            }
            Ok::<(), String>(())
        });
        let diagnostics = Arc::new(Mutex::new(String::new()));
        let error_output = diagnostics.clone();
        let errors = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut stderr = stderr;
            let mut buf = [0; 4096];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut text = error_output.lock().unwrap_or_else(|e| e.into_inner());
                text.push_str(&String::from_utf8_lossy(&buf[..n]));
                if text.len() > 16384 {
                    *text = text
                        .chars()
                        .rev()
                        .take(8192)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                }
            }
        });
        let supervisor_state = state.clone();
        tokio::spawn(async move {
            let group = ProcessGroup(Some(pid));
            let mut reading = reading;
            let mut writing = writing;
            let reason = tokio::select! {
                status = child.wait() => format!("Agent runtime exited: {status:?}"),
                result = &mut reading => format!("Agent output disconnected: {result:?}"),
                result = &mut writing => format!("Agent input disconnected: {result:?}"),
                _ = shutdown_rx.recv() => "Agent runtime stopped".to_owned(),
            };
            drop(group);
            let _ = child.kill().await;
            let _ = child.wait().await;
            reading.abort();
            writing.abort();
            errors.abort();
            let detail = diagnostics
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            supervisor_state.fail(redact_secrets(&format!("{reason} {}", detail.trim())));
        });
        let runtime = Self {
            state,
            writer,
            shutdown,
            session_id: config.session_id.clone(),
            pid,
            home: config.home.clone(),
            mcp_servers: Mutex::new(config.mcp_servers.clone()),
        };
        let initialized = runtime.initialize(&config).await;
        if let Err(error) = initialized {
            runtime.close().await;
            return Err(error);
        }
        Ok(runtime)
    }

    async fn initialize(&self, config: &CliLaunch) -> Result<(), String> {
        self.request("initialize", json!({
            "protocolVersion":1,
            "clientInfo":{"name":"supercharge-e2b-inbox","version":env!("CARGO_PKG_VERSION")},
            "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},
            "_meta":{"clientType":"headless","rules":managed_rules(&config.rules),
                "startupHints":{"nonInteractive":true,"skipGitStatus":true,"skipProjectLayout":true}}
        }), Some(Duration::from_secs(45))).await?;
        let method = if config.resume {
            "session/load"
        } else {
            "session/new"
        };
        let mut params = json!({
            "cwd":config.cwd,"mcpServers":config.mcp_servers,
            "_meta":{"sessionId":config.session_id,"sessionKind":"headless","yoloMode":true,"noReplay":true,
                "modelId":config.model}
        });
        if config.resume {
            params["sessionId"] = json!(config.session_id);
            if let Some(system) = load_system_prompt(&config.home, &config.session_id, &config.cwd)
            {
                params["_meta"]["systemPromptOverride"] =
                    json!(replace_managed_rules(&system, &config.rules));
            } else {
                return Err("Saved conversation has no readable system prompt; cannot safely update its instructions".to_owned());
            }
        }
        self.request(method, params, Some(Duration::from_secs(45)))
            .await?;
        Ok(())
    }

    pub async fn refresh_rules(&self, cwd: &Path, model: &str, rules: &str) -> Result<(), String> {
        if self.is_prompting() {
            return Err("Cannot edit instructions during a turn".into());
        }
        let system = load_system_prompt(&self.home, &self.session_id, cwd)
            .ok_or("Saved system prompt unavailable; instructions were not changed")?;
        let mcp_servers = self
            .mcp_servers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        self.request(
            "session/load",
            json!({"sessionId":self.session_id,"cwd":cwd,"mcpServers":mcp_servers,
            "_meta":{"sessionKind":"headless","yoloMode":true,"noReplay":true,"modelId":model,
            "systemPromptOverride":replace_managed_rules(&system, rules)}}),
            Some(Duration::from_secs(45)),
        )
        .await?;
        Ok(())
    }

    pub fn is_alive(&self) -> bool {
        self.state.alive.load(Ordering::SeqCst)
    }

    pub fn is_prompting(&self) -> bool {
        self.state.prompting.load(Ordering::SeqCst)
    }

    pub async fn update_mcp_servers(&self, servers: Vec<Value>) -> Result<Value, String> {
        if self.is_prompting() {
            return Err("Cannot update MCP servers during a turn".into());
        }
        let result = self
            .request(
                "_x.ai/session/update_mcp_servers",
                json!({"sessionId":self.session_id,"mcpServers":servers}),
                Some(Duration::from_secs(45)),
            )
            .await?;
        *self
            .mcp_servers
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = servers;
        Ok(result)
    }

    pub fn mcp_servers(&self) -> Vec<Value> {
        self.mcp_servers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub async fn wait_until_idle(&self) -> Result<(), String> {
        loop {
            let notified = self.state.settled.notified();
            if !self.is_alive() {
                return Err("Agent runtime is not running".into());
            }
            if !self.is_prompting() {
                return Ok(());
            }
            notified.await;
        }
    }

    pub async fn list_mcp(&self) -> Result<Value, String> {
        self.request(
            "x.ai/mcp/list",
            json!({"sessionId":self.session_id,"cache":false}),
            Some(Duration::from_secs(45)),
        )
        .await
    }

    pub async fn call_mcp(
        &self,
        server: &str,
        tool: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        if self.is_prompting() {
            return Err("Cannot verify MCP during a turn".into());
        }
        self.request(
            "x.ai/mcp/call",
            json!({"sessionId":self.session_id,"server":server,"tool":tool,"arguments":arguments}),
            Some(Duration::from_secs(45)),
        )
        .await
    }

    pub async fn prompt(&self, text: &str, prompt_id: &str) -> Response {
        if self.state.prompting.swap(true, Ordering::SeqCst) {
            return Err("A foreground prompt is already running".to_owned());
        }
        self.request(
            "session/prompt",
            json!({
                "sessionId":self.session_id,"prompt":[{"type":"text","text":text}],
                "_meta":{"promptId":prompt_id,"screenMode":"headless"}
            }),
            None,
        )
        .await
    }

    async fn request(&self, method: &str, params: Value, timeout: Option<Duration>) -> Response {
        if !self.is_alive() {
            return Err("Agent runtime is not running".to_owned());
        }
        let id = self.state.sequence.fetch_add(1, Ordering::Relaxed);
        let (send, receive) = oneshot::channel();
        self.state
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                id,
                PendingRequest {
                    response: send,
                    prompt: method == "session/prompt",
                },
            );
        if self
            .writer
            .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .is_err()
        {
            self.state.fail("Agent input is closed".to_owned());
        }
        let response = async {
            receive
                .await
                .unwrap_or_else(|_| Err("Agent request was interrupted".to_owned()))
        };
        if let Some(timeout) = timeout {
            tokio::time::timeout(timeout, response)
                .await
                .map_err(|_| format!("{method} timed out"))?
        } else {
            response.await
        }
    }

    pub async fn cancel(&self) {
        if !self.state.prompting.load(Ordering::SeqCst) {
            return;
        }
        let _ = self
            .writer
            .send(json!({"jsonrpc":"2.0","method":"session/cancel",
            "params":{"sessionId":self.session_id,"_meta":{"cancelTrigger":"user"}}}));
        let settled = async {
            loop {
                let notified = self.state.settled.notified();
                if !self.state.prompting.load(Ordering::SeqCst) || !self.is_alive() {
                    break;
                }
                notified.await;
            }
        };
        if tokio::time::timeout(Duration::from_secs(10), settled)
            .await
            .is_err()
        {
            self.close().await;
        }
    }

    pub async fn close(&self) {
        let _ = self.shutdown.send(());
        while self.is_alive() {
            let notified = self.state.settled.notified();
            if !self.is_alive() {
                break;
            }
            notified.await;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    async fn runtime(directory: &Path, events: Arc<Mutex<Vec<Value>>>) -> PersistentCli {
        let binary = directory.join("fake-cli");
        std::fs::write(&binary, r##"#!/usr/bin/env python3
import json,sys,threading,time
active=None
count=0
lock=threading.Lock()
def send(data):
    with lock: print(json.dumps(data),flush=True)
def finish(ident,event):
    event.wait(10)
    send({'jsonrpc':'2.0','id':ident,'result':{'stopReason':'cancelled'}})
for line in sys.stdin:
    obj=json.loads(line)
    method=obj.get('method')
    if method=='session/cancel':
        if active: active.set()
        continue
    if method=='session/prompt':
        text=obj['params']['prompt'][0]['text']
        if text=='exit': sys.exit(4)
        if text=='wait':
            active=threading.Event()
            threading.Thread(target=finish,args=(obj['id'],active),daemon=True).start()
            continue
        count+=1
        send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'test','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':str(count)}}}})
    send({'jsonrpc':'2.0','id':obj['id'],'result':{'sessionId':'test','stopReason':'end_turn'}})
"##).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        PersistentCli::launch(
            CliLaunch {
                binary,
                home: directory.to_owned(),
                cwd: directory.to_owned(),
                model: "test".to_owned(),
                session_id: "test".to_owned(),
                resume: false,
                rules: String::new(),
                bot_environment: None,
                mcp_servers: Vec::new(),
                workload_uid: None,
                workload_gid: None,
            },
            Arc::new(move |event| events.lock().unwrap().push(event)),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn consecutive_turns_reuse_process_without_waiting_for_exit() {
        let dir = tempfile::tempdir().unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let runtime = runtime(dir.path(), events.clone()).await;
        let pid = runtime.pid;
        for prompt in ["one", "two"] {
            tokio::time::timeout(Duration::from_secs(2), runtime.prompt(prompt, prompt))
                .await
                .unwrap()
                .unwrap();
            assert!(runtime.is_alive());
            assert_eq!(runtime.pid, pid);
        }
        let text = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| {
                event
                    .pointer("/params/update/content/text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        assert_eq!(text, ["1", "2"]);
        runtime.close().await;
        assert!(!runtime.is_alive());
    }

    #[tokio::test]
    async fn cancel_waits_for_prompt_to_settle_and_preserves_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Arc::new(runtime(dir.path(), Arc::new(Mutex::new(Vec::new()))).await);
        let copy = runtime.clone();
        let prompt = tokio::spawn(async move { copy.prompt("wait", "first").await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        runtime.cancel().await;
        assert!(prompt.await.unwrap().is_ok());
        assert!(runtime.is_alive());
        runtime.prompt("next", "second").await.unwrap();
        runtime.close().await;
    }

    #[tokio::test]
    async fn process_exit_rejects_inflight_request_and_notifies_subscriber() {
        let dir = tempfile::tempdir().unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let runtime = runtime(dir.path(), events.clone()).await;
        assert!(runtime.prompt("exit", "first").await.is_err());
        assert!(!runtime.is_alive());
        assert!(
            events
                .lock()
                .unwrap()
                .iter()
                .any(|event| event["type"] == "runtime_exit")
        );
    }
}

impl Drop for PersistentCli {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
    }
}

pub fn session_directory(home: &Path, session_id: &str, cwd: &Path) -> Option<PathBuf> {
    let encoded = urlencoding::encode(&cwd.to_string_lossy()).into_owned();
    let direct = home.join("sessions").join(encoded).join(session_id);
    if direct.join("summary.json").is_file() {
        return Some(direct);
    }
    for entry in std::fs::read_dir(home.join("sessions")).ok()?.flatten() {
        let directory = entry.path();
        let same_cwd = std::fs::read_to_string(directory.join(".cwd"))
            .ok()
            .is_some_and(|stored| stored.trim() == cwd.to_string_lossy());
        if same_cwd && directory.join(session_id).join("summary.json").is_file() {
            return Some(directory.join(session_id));
        }
    }
    None
}

pub fn session_has_history(home: &Path, session_id: &str, cwd: &Path) -> bool {
    session_directory(home, session_id, cwd).is_some()
}

fn load_system_prompt(home: &Path, session_id: &str, cwd: &Path) -> Option<String> {
    let directory = session_directory(home, session_id, cwd)?;
    if let Ok(system) = std::fs::read_to_string(directory.join("system_prompt.txt")) {
        if !system.trim().is_empty() {
            return Some(system);
        }
    }
    let history = std::fs::read_to_string(directory.join("chat_history.jsonl")).ok()?;
    history
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|item| {
            (item.get("type").and_then(Value::as_str) == Some("system"))
                .then(|| {
                    item.get("content")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .flatten()
        })
}

const BOT_RULES_START: &str = "<!-- vectra-e2b-rules:start -->";
const BOT_RULES_END: &str = "<!-- vectra-e2b-rules:end -->";

fn managed_rules(rules: &str) -> String {
    format!("{BOT_RULES_START}\n{rules}\n{BOT_RULES_END}")
}

fn replace_managed_rules(system: &str, rules: &str) -> String {
    if let Some(start) = system.find(BOT_RULES_START) {
        if let Some(end) = system[start..].find(BOT_RULES_END) {
            let mut next = system.to_owned();
            next.replace_range(
                start..start + end + BOT_RULES_END.len(),
                &managed_rules(rules),
            );
            return next;
        }
    }
    format!("{system}\n\n{}", managed_rules(rules))
}

#[cfg(test)]
mod bot_prompt_tests {
    use super::*;
    #[test]
    fn bot_rules_replace_only_the_managed_block() {
        let old = format!(
            "CLI safety and tools\n{}\nProject instructions",
            managed_rules("old bot rules")
        );
        let updated = replace_managed_rules(&old, "new bot rules");
        assert!(updated.starts_with("CLI safety and tools"));
        assert!(updated.ends_with("Project instructions"));
        assert!(!updated.contains("old bot rules"));
        assert_eq!(updated.matches(BOT_RULES_START).count(), 1);
    }
}
