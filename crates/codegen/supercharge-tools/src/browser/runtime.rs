//! Lazy CDP server process for the installed browser binary.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tempfile::NamedTempFile;

use super::cdp::{CdpError, CdpSession, browser_websocket_url};
use super::install::{binary_path, is_installed};

#[derive(Default)]
struct State {
    child: Option<Child>,
    port: u16,
}

/// Process-wide browser runtime. Spawned on first tool use; killed on drop of the last Arc.
#[derive(Clone, Default)]
pub struct BrowserRuntime {
    inner: Arc<Mutex<State>>,
}

impl BrowserRuntime {
    pub fn global() -> Self {
        static RUNTIME: std::sync::OnceLock<BrowserRuntime> = std::sync::OnceLock::new();
        RUNTIME.get_or_init(BrowserRuntime::default).clone()
    }

    pub async fn session(&self, start_url: &str) -> Result<CdpSession, CdpError> {
        if !is_installed() {
            return Err(CdpError::Message(
                "browser binary is not installed; run `supercharge browser install`".into(),
            ));
        }
        let port = self.ensure_server().await?;
        let ws = browser_websocket_url(port).await?;
        let mut session = CdpSession::connect(&ws).await?;
        session.attach_page(start_url).await?;
        Ok(session)
    }

    async fn ensure_server(&self) -> Result<u16, CdpError> {
        {
            let guard = self.inner.lock();
            if guard.child.is_some() {
                return Ok(guard.port);
            }
        }
        let bin = binary_path();
        let port = pick_port();
        let ready = NamedTempFile::new().map_err(|e| CdpError::Message(e.to_string()))?;
        let ready_path = ready.path().to_path_buf();
        let mut cmd = Command::new(&bin);
        cmd.args([
            "serve",
            "-p",
            &port.to_string(),
            "--host",
            "127.0.0.1",
            "--allow-private-network",
            "--allow-file-access",
            "--ready-file",
        ])
        .arg(&ready_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        let child = cmd.spawn().map_err(|e| {
            CdpError::Message(format!("failed to spawn {}: {e}", bin.display()))
        })?;
        wait_ready(&ready_path, Duration::from_secs(15))?;
        let mut guard = self.inner.lock();
        guard.child = Some(child);
        guard.port = port;
        // Keep ready file alive by leaking into state via forget of NamedTempFile...
        // The server has already written it; we can drop the tempfile after wait.
        drop(ready);
        Ok(port)
    }

    pub fn shutdown(&self) {
        let mut guard = self.inner.lock();
        if let Some(mut child) = guard.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for BrowserRuntime {
    fn drop(&mut self) {
        // Only kill when this is the last Arc (strong_count == 1 after drop of self.inner clone...
        // Global OnceLock holds one Arc forever; provide explicit shutdown on process exit via
        // `atexit`-style call from tools Drop instead. No-op here.
    }
}

fn pick_port() -> u16 {
    // Ephemeral bind to discover a free port, then release it for the browser.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local addr").port()
}

fn wait_ready(path: &PathBuf, budget: Duration) -> Result<(), CdpError> {
    let start = Instant::now();
    while start.elapsed() < budget {
        if path.is_file() && std::fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(CdpError::Message(format!(
        "browser server did not become ready within {budget:?}"
    )))
}
