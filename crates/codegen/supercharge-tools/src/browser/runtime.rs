//! Lazy CDP server process for the installed browser binary.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tempfile::NamedTempFile;

use super::cdp::{browser_websocket_url, CdpError, CdpSession};
use super::install::{binary_path, is_installed};

pub const INSTALL_HINT: &str =
    "local browser binary is not installed; run `supercharge browser install`";

const READY_TIMEOUT: Duration = Duration::from_secs(15);

type LaunchFn = Arc<dyn Fn(u16, &Path) -> std::io::Result<Child> + Send + Sync>;

#[derive(Default)]
struct State {
    child: Option<Child>,
    port: u16,
}

/// Process-wide browser runtime. Spawned on first tool use; killed on drop of the last Arc.
#[derive(Clone)]
pub struct BrowserRuntime {
    inner: Arc<Mutex<State>>,
    page: Arc<tokio::sync::Mutex<Option<CdpSession>>>,
    /// Serializes startup. An async lock so the critical section can await without
    /// holding [`parking_lot::Mutex`] across `.await`.
    startup: Arc<tokio::sync::Mutex<()>>,
    ready_timeout: Duration,
    launch: LaunchFn,
    assume_installed: bool,
}

impl Default for BrowserRuntime {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(State::default())),
            page: Arc::new(tokio::sync::Mutex::new(None)),
            startup: Arc::new(tokio::sync::Mutex::new(())),
            ready_timeout: READY_TIMEOUT,
            launch: Arc::new(default_launch),
            assume_installed: false,
        }
    }
}

impl BrowserRuntime {
    pub fn global() -> Self {
        static RUNTIME: std::sync::OnceLock<BrowserRuntime> = std::sync::OnceLock::new();
        RUNTIME.get_or_init(BrowserRuntime::default).clone()
    }

    pub async fn session(&self, start_url: &str) -> Result<CdpSession, CdpError> {
        self.require_installed()?;
        let port = self.ensure_server().await?;
        let ws = browser_websocket_url(port).await?;
        let mut session = CdpSession::connect(&ws).await?;
        session.attach_page(start_url).await?;
        Ok(session)
    }

    fn require_installed(&self) -> Result<(), CdpError> {
        if self.assume_installed || is_installed() {
            Ok(())
        } else {
            Err(CdpError::Message(INSTALL_HINT.into()))
        }
    }

    /// Open `url` in the process-wide page, creating the CDP session on first use.
    pub async fn open_url(&self, url: &str) -> Result<(), CdpError> {
        self.require_installed()?;
        let port = self.ensure_server().await?;
        let mut page = self.page.lock().await;
        if page.is_none() {
            let ws = browser_websocket_url(port).await?;
            let mut session = CdpSession::connect(&ws).await?;
            session.attach_page(url).await?;
            *page = Some(session);
            return Ok(());
        }
        let session = page
            .as_mut()
            .ok_or_else(|| CdpError::Message("browser page missing after open".into()))?;
        match session.navigate(url).await {
            Ok(()) => Ok(()),
            Err(err) => {
                drop_page_if_broken(&mut page, &err);
                Err(err)
            }
        }
    }

    async fn attached_page(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<CdpSession>>, CdpError> {
        self.require_installed()?;
        let page = self.page.lock().await;
        if page.is_none() {
            return Err(CdpError::Message(
                "no open page; call local_browser_open first".into(),
            ));
        }
        Ok(page)
    }

    pub async fn page_text(&self) -> Result<String, CdpError> {
        let mut page = self.attached_page().await?;
        let session = page.as_mut().ok_or_else(|| {
            CdpError::Message("no open page; call local_browser_open first".into())
        })?;
        match session.page_text().await {
            Ok(value) => Ok(value),
            Err(err) => {
                drop_page_if_broken(&mut page, &err);
                Err(err)
            }
        }
    }

    pub async fn click_selector(&self, selector: &str) -> Result<(), CdpError> {
        let mut page = self.attached_page().await?;
        let session = page.as_mut().ok_or_else(|| {
            CdpError::Message("no open page; call local_browser_open first".into())
        })?;
        match session.click_selector(selector).await {
            Ok(()) => Ok(()),
            Err(err) => {
                drop_page_if_broken(&mut page, &err);
                Err(err)
            }
        }
    }

    pub async fn type_selector(&self, selector: &str, text: &str) -> Result<(), CdpError> {
        let mut page = self.attached_page().await?;
        let session = page.as_mut().ok_or_else(|| {
            CdpError::Message("no open page; call local_browser_open first".into())
        })?;
        match session.type_selector(selector, text).await {
            Ok(()) => Ok(()),
            Err(err) => {
                drop_page_if_broken(&mut page, &err);
                Err(err)
            }
        }
    }

    pub async fn screenshot_png(&self) -> Result<Vec<u8>, CdpError> {
        let mut page = self.attached_page().await?;
        let session = page.as_mut().ok_or_else(|| {
            CdpError::Message("no open page; call local_browser_open first".into())
        })?;
        match session.screenshot_png().await {
            Ok(value) => Ok(value),
            Err(err) => {
                drop_page_if_broken(&mut page, &err);
                Err(err)
            }
        }
    }

    pub async fn evaluate(&self, expression: &str) -> Result<serde_json::Value, CdpError> {
        let mut page = self.attached_page().await?;
        let session = page.as_mut().ok_or_else(|| {
            CdpError::Message("no open page; call local_browser_open first".into())
        })?;
        match session.evaluate(expression).await {
            Ok(value) => Ok(value),
            Err(err) => {
                drop_page_if_broken(&mut page, &err);
                Err(err)
            }
        }
    }

    async fn ensure_server(&self) -> Result<u16, CdpError> {
        let _startup = self.startup.lock().await;
        if let Some(port) = self.live_port() {
            return Ok(port);
        }
        let port = pick_port()?;
        let ready = NamedTempFile::new().map_err(|err| CdpError::Message(err.to_string()))?;
        let ready_path = ready.path().to_path_buf();
        let mut child = (self.launch)(port, &ready_path).map_err(|err| {
            CdpError::Message(format!(
                "failed to spawn browser server on port {port}: {err}"
            ))
        })?;
        match wait_ready(&ready_path, &mut child, self.ready_timeout).await {
            Ok(()) => {
                let mut guard = self.inner.lock();
                guard.port = port;
                guard.child = Some(child);
                Ok(port)
            }
            Err(err) => {
                reap_child(child).await;
                Err(err)
            }
        }
    }

    pub fn shutdown(&self) {
        if let Ok(mut page) = self.page.try_lock() {
            *page = None;
        }
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

fn drop_page_if_broken(page: &mut Option<CdpSession>, err: &CdpError) {
    if err.session_broken() {
        *page = None;
    }
}

impl BrowserRuntime {
    /// Returns the port when the recorded child is still running. Reaps an exited child.
    fn live_port(&self) -> Option<u16> {
        let mut guard = self.inner.lock();
        let child = guard.child.as_mut()?;
        match child.try_wait() {
            Ok(None) => Some(guard.port),
            Ok(Some(_)) => {
                guard.child = None;
                None
            }
            Err(_) => {
                if let Some(mut child) = guard.child.take() {
                    std::thread::spawn(move || {
                        let _ = child.kill();
                        let _ = child.wait();
                    });
                }
                None
            }
        }
    }
}

fn default_launch(port: u16, ready_path: &Path) -> std::io::Result<Child> {
    let bin = binary_path();
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
    .arg(ready_path)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    cmd.spawn()
}

fn pick_port() -> Result<u16, CdpError> {
    pick_port_on("127.0.0.1:0")
}

fn pick_port_on(addr: &str) -> Result<u16, CdpError> {
    let listener = std::net::TcpListener::bind(addr)
        .map_err(|err| CdpError::Message(format!("failed to bind {addr}: {err}")))?;
    listener
        .local_addr()
        .map(|bound| bound.port())
        .map_err(|err| CdpError::Message(format!("failed to read bound port for {addr}: {err}")))
}

async fn wait_ready(path: &Path, child: &mut Child, budget: Duration) -> Result<(), CdpError> {
    let start = Instant::now();
    loop {
        if ready_file_written(path) {
            return Ok(());
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(CdpError::Message(format!(
                "browser server exited before ready: {status}"
            )));
        }
        if start.elapsed() >= budget {
            return Err(CdpError::Message(format!(
                "browser server did not become ready within {budget:?}"
            )));
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

fn ready_file_written(path: &Path) -> bool {
    path.is_file()
        && std::fs::metadata(path)
            .map(|meta| meta.len() > 0)
            .unwrap_or(false)
}

async fn reap_child(mut child: Child) {
    let _ = child.kill();
    let _ = tokio::task::spawn_blocking(move || child.wait()).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    fn test_runtime(
        ready_timeout: Duration,
        launch: impl Fn(u16, &Path) -> std::io::Result<Child> + Send + Sync + 'static,
    ) -> BrowserRuntime {
        BrowserRuntime {
            inner: Arc::new(Mutex::new(State::default())),
            page: Arc::new(tokio::sync::Mutex::new(None)),
            startup: Arc::new(tokio::sync::Mutex::new(())),
            ready_timeout,
            launch: Arc::new(launch),
            assume_installed: true,
        }
    }

    fn pid_alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn spawn_sleep() -> std::io::Result<Child> {
        Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    }

    #[test]
    fn pick_port_reports_bind_failure() {
        let port = pick_port().unwrap();
        assert_ne!(port, 0);
        let err = pick_port_on("256.0.0.1:80").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("failed to bind"), "{text}");
    }

    #[tokio::test]
    async fn concurrent_opens_share_one_child() {
        let starts = Arc::new(AtomicUsize::new(0));
        let starts_launch = starts.clone();
        let runtime = test_runtime(Duration::from_secs(2), move |_port, ready| {
            starts_launch.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(80));
            std::fs::write(ready, b"ready")?;
            spawn_sleep()
        });
        let first = runtime.clone();
        let second = runtime.clone();
        let (left, right) = tokio::join!(first.ensure_server(), second.ensure_server());
        let left = left.unwrap();
        let right = right.unwrap();
        assert_eq!(left, right);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        runtime.shutdown();
    }

    #[tokio::test]
    async fn failed_start_reaps_the_child_and_a_dead_child_is_replaced() {
        let pid = Arc::new(AtomicU32::new(0));
        let pid_launch = pid.clone();
        let runtime = test_runtime(Duration::from_millis(120), move |_port, _ready| {
            let child = spawn_sleep()?;
            pid_launch.store(child.id(), Ordering::SeqCst);
            Ok(child)
        });
        let started = Instant::now();
        let err = runtime.ensure_server().await.unwrap_err();
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "ready wait blocked too long"
        );
        assert!(err.to_string().contains("did not become ready"), "{err}");
        let orphan = pid.load(Ordering::SeqCst);
        assert_ne!(orphan, 0);
        assert!(
            !pid_alive(orphan),
            "failed-start child {orphan} still alive"
        );

        let starts = Arc::new(AtomicUsize::new(0));
        let pids = Arc::new(Mutex::new(Vec::new()));
        let starts_launch = starts.clone();
        let pids_launch = pids.clone();
        let runtime = test_runtime(Duration::from_secs(2), move |_port, ready| {
            starts_launch.fetch_add(1, Ordering::SeqCst);
            std::fs::write(ready, b"ready")?;
            let child = spawn_sleep()?;
            pids_launch.lock().push(child.id());
            Ok(child)
        });
        runtime.ensure_server().await.unwrap();
        let first_pid = pids.lock()[0];
        let _ = std::process::Command::new("kill")
            .arg(first_pid.to_string())
            .status();
        for _ in 0..40 {
            let status =
                std::fs::read_to_string(format!("/proc/{first_pid}/status")).unwrap_or_default();
            if status.is_empty() || status.contains("State:\tZ") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        runtime.ensure_server().await.unwrap();
        assert_eq!(starts.load(Ordering::SeqCst), 2);
        runtime.shutdown();
        assert!(!pid_alive(pids.lock()[1]));
    }

    #[tokio::test]
    async fn early_exit_is_reported() {
        let runtime = test_runtime(Duration::from_secs(2), |_port, _ready| {
            Command::new("sh")
                .arg("-c")
                .arg("exit 3")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
        });
        let err = runtime.ensure_server().await.unwrap_err();
        assert!(err.to_string().contains("exited before ready"), "{err}");
    }

    #[test]
    fn ready_wait_yields_on_the_current_thread_runtime() {
        let progressed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = progressed.clone();
        let runtime = test_runtime(Duration::from_millis(180), |_port, _ready| spawn_sleep());
        let local = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        local.block_on(async {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                flag.store(true, Ordering::SeqCst);
            });
            let err = runtime.ensure_server().await.unwrap_err();
            assert!(err.to_string().contains("did not become ready"), "{err}");
            runtime.shutdown();
        });
        assert!(
            progressed.load(Ordering::SeqCst),
            "ready wait blocked the executor"
        );
    }

    #[tokio::test]
    async fn timed_out_page_call_clears_the_shared_session() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            while socket.next().await.is_some() {
                std::future::pending::<()>().await;
            }
        });
        let session = CdpSession::connect_with_deadlines(
            &format!("ws://127.0.0.1:{port}"),
            super::super::cdp::CdpDeadlines {
                connect: Duration::from_secs(2),
                call: Duration::from_millis(80),
                discovery: Duration::from_millis(80),
            },
        )
        .await
        .unwrap();
        let runtime = test_runtime(Duration::from_millis(50), |_port, _ready| {
            Err(std::io::Error::other("launch should not run"))
        });
        *runtime.page.lock().await = Some(session);
        let started = Instant::now();
        let err = runtime.page_text().await.unwrap_err();
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "page call hung"
        );
        assert!(err.to_string().contains("timed out"), "{err}");
        let next = runtime.page_text().await.unwrap_err();
        assert!(next.to_string().contains("no open page"), "{next}");
    }
}
