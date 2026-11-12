//! Provider sign-in over ACP (`_supercharge/setup/auth_*`).
//!
//! The CLI owns every sign-in: it starts the provider's own device or browser flow, keeps
//! the secret material in process memory, and persists the result to the same place the
//! TUI `/setup` does. The client only receives what it must show the user (a URL and, for
//! device logins, a user code) and a status it can poll. No token, refresh token, API key,
//! PKCE verifier or device code is ever part of a response or notification.
//!
//! | Method | Params | Result |
//! |---|---|---|
//! | `setup/auth_start` | `{providerId, kind?}` | `{loginId, providerId, kind, url, userCode?, expiresIn, interval}` |
//! | `setup/auth_poll` | `{loginId}` | `{loginId, providerId, kind, status, interval, message?}` |
//! | `setup/auth_cancel` | `{loginId}` | `{cancelled}` |
//!
//! Terminal transitions (`done`, `denied`, `expired`, `error`) are also announced with the
//! `_supercharge/setup/auth_changed` notification (same payload as `auth_poll`).
//!
//! One sign-in runs at a time; `auth_start` cancels any sign-in already in progress.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::agent::mistral_browser_sign_in as browser;
use crate::agent::nous_oauth::{self, DevicePollStatus};
use crate::agent::provider_setup as ps;

pub(crate) const AUTH_START: &str = "supercharge/setup/auth_start";
pub(crate) const AUTH_POLL: &str = "supercharge/setup/auth_poll";
pub(crate) const AUTH_CANCEL: &str = "supercharge/setup/auth_cancel";
/// Notification sent when a sign-in reaches a terminal status.
pub(crate) const AUTH_CHANGED: &str = "supercharge/setup/auth_changed";

/// Interactive sign-in kinds the CLI can drive for a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthKind {
    /// Device authorization grant: show `url` and `userCode`, approve on any device.
    Device,
    /// Browser sign-in: open `url` (on any device); the CLI waits for the provider.
    Browser,
}

impl AuthKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            AuthKind::Device => "device",
            AuthKind::Browser => "browser",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "device" | "oauth_device" => Some(AuthKind::Device),
            "browser" => Some(AuthKind::Browser),
            _ => None,
        }
    }
}

/// Sign-in kinds a provider supports over ACP, primary first. Empty for key-only providers.
pub(crate) fn interactive_kinds(provider_id: &str) -> &'static [AuthKind] {
    match provider_id {
        "nous" => &[AuthKind::Device],
        browser::MISTRAL_AUTH_PROVIDER_ID => &[AuthKind::Browser],
        _ => &[],
    }
}

/// Every auth method a listed provider accepts: interactive kinds first, then `api_key`
/// when the provider takes a pasted key.
pub(crate) fn auth_methods(provider_id: &str) -> Vec<&'static str> {
    let kinds = interactive_kinds(provider_id);
    let mut out: Vec<&'static str> = kinds.iter().map(|k| k.as_str()).collect();
    if kinds.is_empty() {
        out.push("api_key");
    }
    out
}

/// Whether the CLI holds a usable sign-in for a provider (`None` for key-only providers).
pub(crate) fn signed_in(provider_id: &str, config: Option<&toml::Value>) -> Option<bool> {
    match provider_id {
        "nous" => Some(nous_oauth::signed_in()),
        browser::MISTRAL_AUTH_PROVIDER_ID => Some(
            config
                .and_then(|c| ps::isolated_provider_api_key(c, provider_id))
                .is_some(),
        ),
        _ => None,
    }
}

/// Terminal or in-progress state of a sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Finish {
    Done,
    Denied(String),
    Expired(String),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    Pending,
    Finished(Finish),
}

/// What a started flow hands back for display. Never carries secret material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Started {
    pub url: String,
    pub user_code: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

type WaitFn = Box<dyn FnOnce(Arc<AtomicBool>, Arc<AtomicU64>) -> Finish + Send>;
type CancelFn = Box<dyn FnOnce() + Send>;
pub(crate) type Notify = Arc<dyn Fn(Value) + Send + Sync>;

/// A started sign-in: display data, a blocking waiter run on its own thread, and an
/// optional hook that stops the provider flow on cancel.
pub(crate) struct Flow {
    pub started: Started,
    pub wait: WaitFn,
    pub on_cancel: Option<CancelFn>,
}

struct Login {
    login_id: String,
    provider_id: String,
    kind: AuthKind,
    interval: Arc<AtomicU64>,
    deadline: Instant,
    phase: Phase,
    cancel: Arc<AtomicBool>,
    on_cancel: Option<CancelFn>,
    reloaded: bool,
}

impl Login {
    fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(hook) = self.on_cancel.take() {
            hook();
        }
    }

    fn view(&self) -> Value {
        let (status, message) = match &self.phase {
            Phase::Pending => ("pending", None),
            Phase::Finished(Finish::Done) => ("done", None),
            Phase::Finished(Finish::Denied(m)) => ("denied", Some(m.clone())),
            Phase::Finished(Finish::Expired(m)) => ("expired", Some(m.clone())),
            Phase::Finished(Finish::Error(m)) => ("error", Some(m.clone())),
        };
        let mut out = json!({
            "loginId": self.login_id,
            "providerId": self.provider_id,
            "kind": self.kind.as_str(),
            "status": status,
            "interval": self.interval.load(Ordering::Relaxed).max(1),
        });
        if let Some(m) = message {
            out["message"] = Value::String(m);
        }
        out
    }
}

fn slot() -> &'static Mutex<Option<Login>> {
    static SLOT: Mutex<Option<Login>> = Mutex::new(None);
    &SLOT
}

fn lock() -> std::sync::MutexGuard<'static, Option<Login>> {
    slot().lock().unwrap_or_else(|e| e.into_inner())
}

/// Errors surfaced as ACP `invalid_params` (client mistakes) or `internal_error` (flow failed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthError {
    Invalid(String),
    Internal(String),
}

/// Validate `{providerId, kind?}` and pick the sign-in kind.
///
/// Rule: an explicit `kind` must be one the provider supports; without it the provider's
/// primary (first advertised) kind is used.
pub(crate) fn choose_kind(provider_id: &str, kind: Option<&str>) -> Result<AuthKind, AuthError> {
    if provider_id.is_empty() {
        return Err(AuthError::Invalid("providerId is required".into()));
    }
    if ps::setup_provider_profiles().all(|p| p.id != provider_id) {
        return Err(AuthError::Invalid(format!(
            "unknown provider `{provider_id}`"
        )));
    }
    let kinds = interactive_kinds(provider_id);
    if kinds.is_empty() {
        return Err(AuthError::Invalid(format!(
            "provider `{provider_id}` does not support sign-in; use setup/save with an apiKey"
        )));
    }
    match kind {
        None => Ok(kinds[0]),
        Some(raw) => {
            let parsed = AuthKind::parse(raw)
                .ok_or_else(|| AuthError::Invalid(format!("unknown auth kind `{raw}`")))?;
            if kinds.contains(&parsed) {
                Ok(parsed)
            } else {
                Err(AuthError::Invalid(format!(
                    "provider `{provider_id}` does not support `{raw}` sign-in"
                )))
            }
        }
    }
}

/// Stop and forget any sign-in in progress.
pub(crate) fn cancel_current() {
    if let Some(mut login) = lock().take() {
        login.stop();
    }
}

/// Register a started flow and run its waiter. Returns the `auth_start` result.
pub(crate) fn install(provider_id: &str, kind: AuthKind, flow: Flow, notify: Notify) -> Value {
    cancel_current();
    let Flow {
        started,
        wait,
        on_cancel,
    } = flow;
    let login_id = format!("login-{}", uuid::Uuid::new_v4().simple());
    let cancel = Arc::new(AtomicBool::new(false));
    let interval = Arc::new(AtomicU64::new(started.interval.max(1)));
    *lock() = Some(Login {
        login_id: login_id.clone(),
        provider_id: provider_id.to_owned(),
        kind,
        interval: Arc::clone(&interval),
        deadline: Instant::now() + Duration::from_secs(started.expires_in.max(1)),
        phase: Phase::Pending,
        cancel: Arc::clone(&cancel),
        on_cancel,
        reloaded: false,
    });
    let thread_id = login_id.clone();
    let spawned = std::thread::Builder::new()
        .name("setup-auth".into())
        .spawn(move || {
            let finish = wait(Arc::clone(&cancel), interval);
            finish_login(&thread_id, finish, &notify);
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "setup auth: could not start the sign-in waiter");
        finish_login(
            &login_id,
            Finish::Error("Could not start sign-in.".into()),
            &(Arc::new(|_| {}) as Notify),
        );
    }
    let mut out = json!({
        "loginId": login_id,
        "providerId": provider_id,
        "kind": kind.as_str(),
        "url": started.url,
        "expiresIn": started.expires_in,
        "interval": started.interval.max(1),
    });
    if let Some(code) = started.user_code {
        out["userCode"] = Value::String(code);
    }
    out
}

/// Record a waiter's result when it still belongs to the current sign-in, then notify.
fn finish_login(login_id: &str, finish: Finish, notify: &Notify) {
    let view = {
        let mut guard = lock();
        let Some(login) = guard.as_mut().filter(|l| l.login_id == login_id) else {
            return; // cancelled or replaced: drop the result
        };
        if login.phase != Phase::Pending {
            return;
        }
        login.phase = Phase::Finished(finish);
        login.on_cancel = None;
        login.view()
    };
    notify(view);
}

/// `auth_poll`. Returns the status and whether the caller should hot-reload the model
/// catalog (first observation of `done`).
pub(crate) fn poll(login_id: &str, notify: &Notify) -> Result<(Value, bool), AuthError> {
    if login_id.is_empty() {
        return Err(AuthError::Invalid("loginId is required".into()));
    }
    let (view, reload, expired_now) = {
        let mut guard = lock();
        let login = guard
            .as_mut()
            .filter(|l| l.login_id == login_id)
            .ok_or_else(|| {
                AuthError::Invalid("unknown or cancelled loginId; call setup/auth_start".into())
            })?;
        let mut expired_now = false;
        if login.phase == Phase::Pending && Instant::now() >= login.deadline {
            login.stop();
            login.phase = Phase::Finished(Finish::Expired("Sign-in expired. Start again.".into()));
            expired_now = true;
        }
        let reload = login.phase == Phase::Finished(Finish::Done) && !login.reloaded;
        if reload {
            login.reloaded = true;
        }
        (login.view(), reload, expired_now)
    };
    if expired_now {
        notify(view.clone());
    }
    Ok((view, reload))
}

/// `auth_cancel`: `true` when a matching sign-in was stopped.
pub(crate) fn cancel(login_id: &str) -> Result<bool, AuthError> {
    if login_id.is_empty() {
        return Err(AuthError::Invalid("loginId is required".into()));
    }
    let mut guard = lock();
    if guard.as_ref().is_some_and(|l| l.login_id == login_id) {
        if let Some(mut login) = guard.take() {
            login.stop();
        }
        return Ok(true);
    }
    Ok(false)
}

/// Summary of the current sign-in for `setup/status` (`null` when none).
pub(crate) fn current_view() -> Value {
    lock().as_ref().map(Login::view).unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// Production flows: the same provider flows the TUI `/setup` drives.

fn interruptible_sleep(cancel: &AtomicBool, secs: u64) {
    let mut left = Duration::from_secs(secs.max(1));
    let slice = Duration::from_millis(200);
    while !left.is_zero() && !cancel.load(Ordering::Relaxed) {
        let step = slice.min(left);
        std::thread::sleep(step);
        left = left.saturating_sub(step);
    }
}

fn classify_device_error(message: &str) -> Finish {
    if message.contains("access_denied") {
        Finish::Denied("Sign-in was denied.".into())
    } else if message.contains("expired_token") {
        Finish::Expired("Sign-in expired. Start again.".into())
    } else {
        Finish::Error(message.to_owned())
    }
}

/// Device sign-in: request a code now; the waiter polls the token endpoint and saves the
/// token where the CLI keeps it.
fn device_flow() -> Result<Flow, AuthError> {
    let pending = nous_oauth::request_device_code().map_err(AuthError::Internal)?;
    let url = if pending.verification_uri_complete.is_empty() {
        pending.verification_uri.clone()
    } else {
        pending.verification_uri_complete.clone()
    };
    let started = Started {
        url,
        user_code: Some(pending.user_code.clone()),
        expires_in: pending.expires_in,
        interval: pending.interval.max(1),
    };
    let expires = Duration::from_secs(pending.expires_in.max(1));
    let wait: WaitFn = Box::new(move |cancel, interval| {
        let deadline = Instant::now() + expires;
        let mut transport_failures = 0u32;
        loop {
            interruptible_sleep(&cancel, interval.load(Ordering::Relaxed));
            if cancel.load(Ordering::Relaxed) {
                return Finish::Error("Sign-in was cancelled.".into());
            }
            if Instant::now() >= deadline {
                return Finish::Expired("Sign-in expired. Start again.".into());
            }
            match nous_oauth::poll_once(&pending) {
                Ok(DevicePollStatus::Pending) => transport_failures = 0,
                Ok(DevicePollStatus::SlowDown) => {
                    transport_failures = 0;
                    interval.fetch_add(5, Ordering::Relaxed);
                }
                Ok(DevicePollStatus::Done) => return Finish::Done,
                Err(message) if message.contains("poll failed") && transport_failures < 3 => {
                    transport_failures += 1;
                }
                Err(message) => return classify_device_error(&message),
            }
        }
    });
    Ok(Flow {
        started,
        wait,
        on_cancel: None,
    })
}

fn classify_browser_error(error: &browser::SignInError) -> Finish {
    use browser::SignInCode as C;
    match error.code() {
        C::Denied => Finish::Denied(error.to_string()),
        C::Expired | C::TimedOut => Finish::Expired(error.to_string()),
        _ => Finish::Error(error.to_string()),
    }
}

/// Browser sign-in: start the provider's PKCE process now; the waiter polls it, then saves
/// the issued key the way the CLI's own sign-in does.
fn browser_flow() -> Result<Flow, AuthError> {
    let generation = loop {
        let g = uuid::Uuid::new_v4().as_u64_pair().0;
        if g != browser::HEADLESS_GENERATION {
            break g;
        }
    };
    let url = browser::begin_live(generation).map_err(|e| AuthError::Internal(e.to_string()))?;
    let expires_in = browser::live_expires_at(generation)
        .map(|at| (at - chrono::Utc::now()).num_seconds().max(1) as u64)
        .unwrap_or(15 * 60);
    let started = Started {
        url,
        user_code: None,
        expires_in,
        interval: browser::poll_interval_secs(),
    };
    let wait: WaitFn = Box::new(
        move |_cancel, _interval| match browser::wait_live(generation) {
            Ok(()) => match browser::take_api_key(generation) {
                Some(key) => match browser::persist_issued_key(&key, None) {
                    Ok(_) => Finish::Done,
                    Err(e) => Finish::Error(e.to_string()),
                },
                None => Finish::Error("Sign-in finished without a key.".into()),
            },
            Err(e) => classify_browser_error(&e),
        },
    );
    Ok(Flow {
        started,
        wait,
        on_cancel: Some(Box::new(move || browser::cancel_live(generation))),
    })
}

/// Start the provider's real flow for `kind`. Blocking (network); run off the async runtime.
pub(crate) fn production_flow(provider_id: &str, kind: AuthKind) -> Result<Flow, AuthError> {
    match (provider_id, kind) {
        ("nous", AuthKind::Device) => device_flow(),
        (browser::MISTRAL_AUTH_PROVIDER_ID, AuthKind::Browser) => browser_flow(),
        _ => Err(AuthError::Invalid(format!(
            "provider `{provider_id}` does not support `{}` sign-in",
            kind.as_str()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    const SECRET: &str = "sk-very-secret-token-value";

    fn collector() -> (Notify, mpsc::Receiver<Value>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        (
            Arc::new(move |v| {
                let _ = tx.lock().unwrap().send(v);
            }),
            rx,
        )
    }

    /// A flow whose waiter finishes when the test sends a result (or on cancel).
    fn fake_flow(
        user_code: Option<&str>,
        expires_in: u64,
    ) -> (Flow, mpsc::Sender<Finish>, Arc<AtomicBool>) {
        let (tx, rx) = mpsc::channel::<Finish>();
        let hook_called = Arc::new(AtomicBool::new(false));
        let hook = Arc::clone(&hook_called);
        let flow = Flow {
            started: Started {
                url: "https://sign-in.example.test/device?code=ABCD".into(),
                user_code: user_code.map(str::to_owned),
                expires_in,
                interval: 2,
            },
            wait: Box::new(move |cancel, _| {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return Finish::Error("cancelled".into());
                    }
                    match rx.recv_timeout(Duration::from_millis(20)) {
                        Ok(f) => return f,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(_) => return Finish::Error("gone".into()),
                    }
                }
            }),
            on_cancel: Some(Box::new(move || hook.store(true, Ordering::Relaxed))),
        };
        (flow, tx, hook_called)
    }

    fn wait_status(login_id: &str, notify: &Notify, want: &str) -> Value {
        for _ in 0..200 {
            let (v, _) = poll(login_id, notify).unwrap();
            if v["status"] == want {
                return v;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("status never became {want}");
    }

    #[test]
    #[serial_test::serial(setup_auth)]
    fn start_returns_url_and_user_code_then_done_is_polled_and_notified() {
        let (notify, rx) = collector();
        let (flow, finish, _) = fake_flow(Some("ABCD-EFGH"), 600);
        let out = install("nous", AuthKind::Device, flow, Arc::clone(&notify));
        assert_eq!(out["kind"], "device");
        assert_eq!(out["providerId"], "nous");
        assert_eq!(out["url"], "https://sign-in.example.test/device?code=ABCD");
        assert_eq!(out["userCode"], "ABCD-EFGH");
        assert_eq!(out["expiresIn"], 600);
        assert_eq!(out["interval"], 2);
        let id = out["loginId"].as_str().unwrap().to_owned();
        assert!(id.starts_with("login-"));

        let (v, reload) = poll(&id, &notify).unwrap();
        assert_eq!(v["status"], "pending");
        assert!(!reload);
        assert_eq!(current_view()["status"], "pending");

        finish.send(Finish::Done).unwrap();
        let v = wait_status(&id, &notify, "done");
        assert!(v.get("message").is_none());
        let note = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(note["status"], "done");
        assert_eq!(note["loginId"], id.as_str());
        // Catalog reload is requested once, and `done` stays readable.
        let (_, reload_again) = poll(&id, &notify).unwrap();
        assert!(!reload_again);
        cancel_current();
    }

    #[test]
    #[serial_test::serial(setup_auth)]
    fn browser_start_has_no_user_code() {
        let (notify, _rx) = collector();
        let (flow, _finish, _) = fake_flow(None, 900);
        let out = install("mistral-auth", AuthKind::Browser, flow, notify);
        assert_eq!(out["kind"], "browser");
        assert!(out.get("userCode").is_none());
        assert!(out["url"].as_str().unwrap().starts_with("https://"));
        cancel_current();
    }

    #[test]
    #[serial_test::serial(setup_auth)]
    fn cancel_stops_flow_and_poll_fails_closed() {
        let (notify, rx) = collector();
        let (flow, finish, hook) = fake_flow(Some("X"), 600);
        let id = install("nous", AuthKind::Device, flow, Arc::clone(&notify))["loginId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(!cancel("login-other").unwrap());
        assert!(cancel(&id).unwrap());
        assert!(hook.load(Ordering::Relaxed));
        assert!(matches!(poll(&id, &notify), Err(AuthError::Invalid(_))));
        assert!(!cancel(&id).unwrap());
        // A late result from the cancelled waiter is dropped, not announced.
        let _ = finish.send(Finish::Done);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(current_view(), Value::Null);
    }

    #[test]
    #[serial_test::serial(setup_auth)]
    fn new_start_replaces_previous_login() {
        let (notify, _rx) = collector();
        let (first, _f1, hook1) = fake_flow(Some("A"), 600);
        let id1 = install("nous", AuthKind::Device, first, Arc::clone(&notify))["loginId"]
            .as_str()
            .unwrap()
            .to_owned();
        let (second, _f2, _) = fake_flow(Some("B"), 600);
        let id2 = install("nous", AuthKind::Device, second, Arc::clone(&notify))["loginId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(id1, id2);
        assert!(hook1.load(Ordering::Relaxed));
        assert!(poll(&id1, &notify).is_err());
        assert_eq!(poll(&id2, &notify).unwrap().0["status"], "pending");
        cancel_current();
    }

    #[test]
    #[serial_test::serial(setup_auth)]
    fn deadline_expires_login_and_notifies() {
        let (notify, rx) = collector();
        let (flow, _finish, hook) = fake_flow(Some("A"), 1);
        let id = install("nous", AuthKind::Device, flow, Arc::clone(&notify))["loginId"]
            .as_str()
            .unwrap()
            .to_owned();
        std::thread::sleep(Duration::from_millis(1100));
        let (v, reload) = poll(&id, &notify).unwrap();
        assert_eq!(v["status"], "expired");
        assert!(v["message"].is_string());
        assert!(!reload);
        assert!(hook.load(Ordering::Relaxed));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap()["status"],
            "expired"
        );
        cancel_current();
    }

    #[test]
    #[serial_test::serial(setup_auth)]
    fn denied_and_error_are_reported() {
        for (finish, want) in [
            (Finish::Denied("Sign-in was denied.".into()), "denied"),
            (Finish::Error("provider unavailable".into()), "error"),
            (Finish::Expired("expired".into()), "expired"),
        ] {
            let (notify, rx) = collector();
            let (flow, tx, _) = fake_flow(Some("A"), 600);
            let id = install("nous", AuthKind::Device, flow, Arc::clone(&notify))["loginId"]
                .as_str()
                .unwrap()
                .to_owned();
            tx.send(finish).unwrap();
            let v = wait_status(&id, &notify, want);
            assert!(v["message"].is_string());
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(2)).unwrap()["status"],
                want
            );
        }
        cancel_current();
    }

    #[test]
    fn kind_selection_fails_closed() {
        assert_eq!(choose_kind("nous", None).unwrap(), AuthKind::Device);
        assert_eq!(
            choose_kind("nous", Some("device")).unwrap(),
            AuthKind::Device
        );
        assert_eq!(
            choose_kind("mistral-auth", None).unwrap(),
            AuthKind::Browser
        );
        assert!(matches!(
            choose_kind("nous", Some("browser")),
            Err(AuthError::Invalid(_))
        ));
        assert!(matches!(
            choose_kind("nous", Some("telepathy")),
            Err(AuthError::Invalid(_))
        ));
        assert!(matches!(
            choose_kind("openai", None),
            Err(AuthError::Invalid(_))
        ));
        assert!(matches!(
            choose_kind("no-such-provider", None),
            Err(AuthError::Invalid(_))
        ));
        assert!(matches!(choose_kind("", None), Err(AuthError::Invalid(_))));
        // Hidden profiles are not startable.
        assert!(matches!(
            choose_kind("codex", None),
            Err(AuthError::Invalid(_))
        ));
        assert!(matches!(
            poll("", &(Arc::new(|_| {}) as Notify)),
            Err(AuthError::Invalid(_))
        ));
        assert!(matches!(cancel(""), Err(AuthError::Invalid(_))));
    }

    #[test]
    fn auth_methods_per_provider() {
        assert_eq!(auth_methods("nous"), vec!["device"]);
        assert_eq!(auth_methods("mistral-auth"), vec!["browser"]);
        assert_eq!(auth_methods("openai"), vec!["api_key"]);
        assert_eq!(signed_in("openai", None), None);
        let cfg: toml::Value = toml::from_str(&format!(
            "[provider_credentials.mistral-auth]\napi_key = \"{SECRET}\"\n"
        ))
        .unwrap();
        assert_eq!(signed_in("mistral-auth", Some(&cfg)), Some(true));
        assert_eq!(signed_in("mistral-auth", None), Some(false));
    }

    #[test]
    fn device_and_browser_errors_classify() {
        assert!(matches!(
            classify_device_error("Nous login access_denied"),
            Finish::Denied(_)
        ));
        assert!(matches!(
            classify_device_error("Nous login expired_token"),
            Finish::Expired(_)
        ));
        assert!(matches!(
            classify_device_error("HTTP 500"),
            Finish::Error(_)
        ));
    }

    #[test]
    #[serial_test::serial(setup_auth)]
    fn responses_carry_no_secret_material() {
        let (notify, rx) = collector();
        let (flow, tx, _) = fake_flow(Some("ABCD"), 600);
        let start = install("nous", AuthKind::Device, flow, Arc::clone(&notify));
        let id = start["loginId"].as_str().unwrap().to_owned();
        tx.send(Finish::Done).unwrap();
        let done = wait_status(&id, &notify, "done");
        let note = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        for v in [&start, &done, &note, &current_view()] {
            let text = v.to_string();
            assert!(!text.contains(SECRET));
            for forbidden in [
                "token",
                "apiKey",
                "api_key",
                "verifier",
                "deviceCode",
                "device_code",
            ] {
                assert!(!text.contains(forbidden), "{forbidden} leaked in {text}");
            }
        }
        cancel_current();
    }
}
