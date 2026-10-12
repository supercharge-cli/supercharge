use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const CLEANUP_GRACE: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(super) struct SignalHandoff {
    code: AtomicI32,
    armed: AtomicBool,
    notify: Notify,
    pub(super) cancel: parking_lot::Mutex<Option<CancellationToken>>,
}

pub(super) struct HeadlessSignals {
    pub(super) handoff: Arc<SignalHandoff>,
}

pub(super) struct DeferredExit {
    handoff: Arc<SignalHandoff>,
}

impl HeadlessSignals {
    pub(super) fn defer_exit(&self) -> DeferredExit {
        self.handoff.armed.store(true, Ordering::Release);
        DeferredExit {
            handoff: self.handoff.clone(),
        }
    }

    pub(super) fn install(cancel: CancellationToken) -> anyhow::Result<Self> {
        let handoff = Arc::new(SignalHandoff::default());
        *handoff.cancel.lock() = Some(cancel);
        #[cfg(unix)]
        {
            let mut signals = signal_hook::iterator::Signals::new([
                signal_hook::consts::SIGINT,
                signal_hook::consts::SIGTERM,
                signal_hook::consts::SIGHUP,
            ])?;
            let state = handoff.clone();
            std::thread::Builder::new()
                .name("headless-signals".into())
                .spawn(move || {
                    for sig in signals.forever() {
                        let code = 128 + sig;
                        match route_signal(&state, code) {
                            SignalRoute::Pending => arm_watchdog(state.clone(), code),
                            SignalRoute::Force(code) => std::process::exit(code),
                        }
                    }
                })?;
        }
        #[cfg(not(unix))]
        {
            let state = handoff.clone();
            std::thread::Builder::new()
                .name("headless-signals".into())
                .spawn(move || {
                    if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        runtime.block_on(async {
                            loop {
                                if tokio::signal::ctrl_c().await.is_err() {
                                    break;
                                }
                                match route_signal(&state, 130) {
                                    SignalRoute::Pending => arm_watchdog(state.clone(), 130),
                                    SignalRoute::Force(code) => std::process::exit(code),
                                }
                            }
                        });
                    }
                })?;
        }
        Ok(Self { handoff })
    }
}

fn arm_watchdog(state: Arc<SignalHandoff>, code: i32) {
    std::thread::spawn(move || {
        std::thread::sleep(CLEANUP_GRACE);
        if state.code.load(Ordering::Acquire) != 0 {
            std::process::exit(code);
        }
    });
}

impl DeferredExit {
    pub(super) async fn signalled(&self) {
        loop {
            let notified = self.handoff.notify.notified();
            if self.handoff.code.load(Ordering::Acquire) != 0 {
                return;
            }
            notified.await;
        }
    }

    pub(super) fn release(&self) -> Option<i32> {
        self.handoff.armed.store(false, Ordering::Release);
        let code = self.handoff.code.swap(0, Ordering::AcqRel);
        (code != 0).then_some(code)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SignalRoute {
    Pending,
    Force(i32),
}

pub(super) fn route_signal(state: &SignalHandoff, code: i32) -> SignalRoute {
    if !state.armed.load(Ordering::Acquire) {
        return SignalRoute::Force(code);
    }
    match state
        .code
        .compare_exchange(0, code, Ordering::AcqRel, Ordering::Acquire)
    {
        Ok(_) => {
            if let Some(cancel) = state.cancel.lock().as_ref() {
                cancel.cancel();
            }
            state.notify.notify_waiters();
            SignalRoute::Pending
        }
        Err(_) => SignalRoute::Force(code),
    }
}

#[cfg(test)]
pub(super) trait SignalSource {
    async fn next_code(&mut self) -> i32;
}

#[cfg(test)]
pub(super) async fn forced_exit_code<S: SignalSource>(
    source: &mut S,
    state: &SignalHandoff,
) -> i32 {
    loop {
        let code = source.next_code().await;
        if let SignalRoute::Force(code) = route_signal(state, code) {
            return code;
        }
    }
}

#[cfg(test)]
pub(super) async fn spawn_watcher<S, F, E>(
    source: F,
    state: Arc<SignalHandoff>,
    exit: E,
) -> std::io::Result<()>
where
    S: SignalSource + Send + 'static,
    F: FnOnce() -> S + Send + 'static,
    E: FnOnce(i32) + Send + 'static,
{
    std::thread::Builder::new()
        .name("headless-test-signal".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let mut source = source();
            let code = runtime.block_on(forced_exit_code(&mut source, &state));
            exit(code);
        })?;
    Ok(())
}
