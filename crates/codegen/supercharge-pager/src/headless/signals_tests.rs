use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use tokio::sync::mpsc;

use super::{
    HeadlessSignals, SignalRoute, SignalSource, forced_exit_code, route_signal, spawn_watcher,
};

impl SignalSource for mpsc::UnboundedReceiver<i32> {
    async fn next_code(&mut self) -> i32 {
        self.recv().await.expect("sender still alive")
    }
}

fn uninstalled() -> HeadlessSignals {
    HeadlessSignals {
        handoff: Arc::default(),
    }
}

#[test]
fn deferred_signal_wakes_and_releases_original_exit_code() {
    let signals = uninstalled();
    let deferred = signals.defer_exit();
    let mut notified = pin!(deferred.signalled());
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(notified.as_mut().poll(&mut cx), Poll::Pending);
    assert_eq!(route_signal(&signals.handoff, 143), SignalRoute::Pending);
    assert_eq!(notified.as_mut().poll(&mut cx), Poll::Ready(()));
    drop(notified);
    assert_eq!(deferred.release(), Some(143));
}

#[tokio::test(start_paused = true)]
async fn second_signal_forces_exit_during_cleanup() {
    let signals = uninstalled();
    let _deferred = signals.defer_exit();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut forced = pin!(forced_exit_code(&mut rx, &signals.handoff));
    tx.send(143).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), forced.as_mut())
            .await
            .is_err()
    );
    tx.send(130).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), forced)
            .await
            .unwrap(),
        130
    );
}

#[tokio::test]
async fn first_signal_cancels_before_cleanup_releases_exit() {
    let cancel = tokio_util::sync::CancellationToken::new();
    let signals = uninstalled();
    *signals.handoff.cancel.lock() = Some(cancel.clone());
    let deferred = signals.defer_exit();
    assert_eq!(route_signal(&signals.handoff, 130), SignalRoute::Pending);
    deferred.signalled().await;
    assert!(cancel.is_cancelled());
    assert_eq!(deferred.release(), Some(130));
}

#[cfg(unix)]
#[test]
fn subprocess_signal_helper() {
    if std::env::var_os("SUPERCHARGE_HEADLESS_SIGNAL_TEST_CHILD").is_none() {
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let signals = HeadlessSignals::install(tokio_util::sync::CancellationToken::new()).unwrap();
        let deferred = signals.defer_exit();
        use std::io::Write;
        std::io::stdout().write_all(b"ready\n").unwrap();
        std::io::stdout().flush().unwrap();
        deferred.signalled().await;
        std::process::exit(deferred.release().unwrap());
    });
}

#[cfg(unix)]
#[test]
fn subprocess_sigterm_exits_without_tui_output() {
    use std::io::BufRead;
    use std::process::{Command, Stdio};
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("headless::signals_tests::subprocess_signal_helper")
        .arg("--nocapture")
        .env("SUPERCHARGE_HEADLESS_SIGNAL_TEST_CHILD", "1")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        assert_ne!(
            stdout.read_line(&mut line).unwrap(),
            0,
            "child exited before ready"
        );
        if line.contains("ready") {
            break;
        }
        line.clear();
    }
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(143));
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut stdout, &mut rest).unwrap();
    assert!(!rest.contains('\u{1b}'));
}

#[tokio::test(flavor = "current_thread")]
async fn watcher_exits_even_when_turn_worker_is_blocked() {
    let (tx, rx) = mpsc::unbounded_channel();
    let (exit_tx, exit_rx) = std::sync::mpsc::channel();
    spawn_watcher(
        move || rx,
        Arc::default(),
        move |code| {
            exit_tx.send(code).unwrap();
        },
    )
    .await
    .unwrap();
    tx.send(129).unwrap();
    assert_eq!(exit_rx.recv_timeout(Duration::from_secs(5)).unwrap(), 129);
}
