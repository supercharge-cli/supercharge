//! Anonymous product usage statistics.
//!
//! A small, self-contained pipeline that tells the Supercharge team how the CLI
//! is used: active installs, versions, platforms, which commands run, how many
//! sessions start, which provider/API style serves them, ACP (editor) usage, and
//! coarse error categories.
//!
//! Privacy contract (enforced by the typed [`UsageEvent`] API and by tests):
//! - The only identifier is a random UUID stored in `<supercharge home>/usage-stats.json`.
//! - No usernames, hostnames, paths, emails, IP addresses, prompts, code, file
//!   contents, API keys, or model output are ever attached. Event properties
//!   are built only from enums, static labels, and sanitized identifiers.
//! - Sending is fire-and-forget on a dedicated background thread with short
//!   timeouts; failures are silent and never affect the CLI.
//!
//! Opt out with `supercharge telemetry off`, `SUPERCHARGE_TELEMETRY=0`,
//! `DO_NOT_TRACK=1`, or `[telemetry] usage_stats = false` in `config.toml`.
//! Nothing is sent from CI environments, dev builds, or unit tests.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Public, write-only ingestion token for the Supercharge analytics project.
/// It can only append events; it cannot read data. Override at build time with
/// `SUPERCHARGE_POSTHOG_KEY` or at run time with the same env var.
const DEFAULT_PROJECT_KEY: &str = "phc_zQ4JMELY4e6vfQbfBAJdyGMHgqv9M2A4MYqtmRLtUHhY";
const DEFAULT_HOST: &str = "https://us.i.posthog.com";
/// Name of the persisted state file inside the Supercharge home.
pub const STATE_FILE_NAME: &str = "usage-stats.json";
/// Env var: `0`/`false`/`off` disables; `1`/`true`/`on` force-enables (even in CI or dev builds).
pub const ENV_SWITCH: &str = "SUPERCHARGE_TELEMETRY";
/// Set on child processes so they don't re-report `app_started` / `command_run`.
const CHILD_MARKER_ENV: &str = "SUPERCHARGE_USAGE_STATS_CHILD";

const BATCH_WINDOW: Duration = Duration::from_secs(2);
const MAX_BATCH: usize = 25;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Upper bound on queued events, so a dead network can never grow memory.
const MAX_QUEUED: usize = 200;

/// The project key compiled into this build (env override at build time).
fn compiled_project_key() -> &'static str {
    match option_env!("SUPERCHARGE_POSTHOG_KEY") {
        Some(key) if !key.trim().is_empty() => key,
        _ => DEFAULT_PROJECT_KEY,
    }
}

fn project_key() -> String {
    std::env::var("SUPERCHARGE_POSTHOG_KEY")
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| compiled_project_key().to_owned())
}

fn ingestion_host() -> String {
    std::env::var("SUPERCHARGE_POSTHOG_HOST")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_owned())
        .filter(|v| v.starts_with("https://") || is_loopback_http(v))
        .unwrap_or_else(|| DEFAULT_HOST.to_owned())
}

/// Plain HTTP is only accepted for a local test collector.
fn is_loopback_http(url: &str) -> bool {
    ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"]
        .iter()
        .any(|prefix| url.starts_with(prefix))
}

// ---------------------------------------------------------------------------
// Opt-out resolution
// ---------------------------------------------------------------------------

/// Why usage stats are on or off. Shown by `supercharge telemetry status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Enabled,
    /// `SUPERCHARGE_TELEMETRY=1` forced it on.
    EnabledByEnv,
    DisabledByEnv,
    DisabledByDoNotTrack,
    DisabledByCommand,
    DisabledByConfig,
    DisabledInCi,
    DisabledDevBuild,
    DisabledNoKey,
}

impl Decision {
    pub fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled | Self::EnabledByEnv)
    }

    /// Human-readable reason for `supercharge telemetry status`.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Enabled => "on",
            Self::EnabledByEnv => "on (forced by SUPERCHARGE_TELEMETRY=1)",
            Self::DisabledByEnv => "off (SUPERCHARGE_TELEMETRY / DISABLE_TELEMETRY env)",
            Self::DisabledByDoNotTrack => "off (DO_NOT_TRACK env)",
            Self::DisabledByCommand => "off (supercharge telemetry off)",
            Self::DisabledByConfig => "off ([telemetry] usage_stats = false in config.toml)",
            Self::DisabledInCi => "off (CI environment detected)",
            Self::DisabledDevBuild => "off (development build)",
            Self::DisabledNoKey => "off (no analytics key in this build)",
        }
    }
}

/// Inputs to [`decide`], gathered from the environment and disk so the policy
/// itself is a pure, testable function.
#[derive(Debug, Clone, Default)]
pub struct Signals {
    pub telemetry_env: Option<String>,
    pub do_not_track_env: Option<String>,
    pub disable_telemetry_env: Option<String>,
    pub ci: bool,
    pub dev_build: bool,
    pub key_present: bool,
    /// `supercharge telemetry on|off` (persisted in the state file).
    pub command_setting: Option<bool>,
    /// `[telemetry] usage_stats` in config.toml.
    pub config_setting: Option<bool>,
}

fn parse_switch(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Some(true),
        "0" | "false" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
}

/// `DO_NOT_TRACK` follows the consoledonottrack.com convention: any value other
/// than empty / `0` / `false` means "do not track".
fn do_not_track_set(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        None | Some("") => false,
        Some(v) => parse_switch(v) != Some(false),
    }
}

/// Opt-out policy. Explicit opt-outs always win; `SUPERCHARGE_TELEMETRY=1` is the
/// only thing that can re-enable collection in CI or a dev build.
pub fn decide(s: &Signals) -> Decision {
    if !s.key_present {
        return Decision::DisabledNoKey;
    }
    if do_not_track_set(s.do_not_track_env.as_deref()) {
        return Decision::DisabledByDoNotTrack;
    }
    if s.disable_telemetry_env
        .as_deref()
        .and_then(parse_switch)
        .unwrap_or(false)
    {
        return Decision::DisabledByEnv;
    }
    let env_switch = s.telemetry_env.as_deref().and_then(parse_switch);
    if env_switch == Some(false) {
        return Decision::DisabledByEnv;
    }
    if env_switch == Some(true) {
        return Decision::EnabledByEnv;
    }
    if s.command_setting == Some(false) {
        return Decision::DisabledByCommand;
    }
    if s.config_setting == Some(false) {
        return Decision::DisabledByConfig;
    }
    if s.ci {
        return Decision::DisabledInCi;
    }
    if s.dev_build {
        return Decision::DisabledDevBuild;
    }
    Decision::Enabled
}

const CI_ENV_VARS: &[&str] = &[
    "CI",
    "CONTINUOUS_INTEGRATION",
    "GITHUB_ACTIONS",
    "GITLAB_CI",
    "BUILDKITE",
    "CIRCLECI",
    "TRAVIS",
    "JENKINS_URL",
    "TF_BUILD",
    "TEAMCITY_VERSION",
    "BITBUCKET_BUILD_NUMBER",
    "CODEBUILD_BUILD_ID",
    "NEXTEST",
    "RUST_TEST_THREADS",
];

fn detect_ci() -> bool {
    CI_ENV_VARS.iter().any(|name| {
        std::env::var(name)
            .ok()
            .is_some_and(|v| !v.trim().is_empty() && parse_switch(&v) != Some(false))
    })
}

fn gather_signals(state: &PersistedState, config_setting: Option<bool>) -> Signals {
    Signals {
        telemetry_env: std::env::var(ENV_SWITCH).ok(),
        do_not_track_env: std::env::var("DO_NOT_TRACK").ok(),
        disable_telemetry_env: std::env::var("DISABLE_TELEMETRY").ok(),
        // Unit tests of this crate behave like CI: never send.
        ci: detect_ci() || cfg!(test),
        dev_build: supercharge_version::IS_DEV_BUILD,
        key_present: !project_key().is_empty(),
        command_setting: state.enabled,
        config_setting,
    }
}

// ---------------------------------------------------------------------------
// Persisted state (anonymous id + on/off + notice flag)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistedState {
    /// Random v4 UUID; the only identifier ever sent.
    pub anonymous_id: Option<String>,
    /// Set by `supercharge telemetry on|off`; `None` = default.
    pub enabled: Option<bool>,
    /// Whether the one-time first-run notice was shown.
    pub notice_shown: bool,
}

fn default_state_path() -> PathBuf {
    supercharge_dirs::supercharge_home().join(STATE_FILE_NAME)
}

pub fn load_state_from(path: &Path) -> PersistedState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save_state_to(path: &Path, state: &PersistedState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

fn valid_uuid(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok()
}

/// Returns the persisted anonymous id, creating (and saving) one if missing.
fn ensure_anonymous_id(path: &Path, state: &mut PersistedState) -> String {
    if let Some(id) = state.anonymous_id.as_deref().filter(|id| valid_uuid(id)) {
        return id.to_owned();
    }
    let id = uuid::Uuid::new_v4().to_string();
    state.anonymous_id = Some(id.clone());
    let _ = save_state_to(path, state);
    id
}

// ---------------------------------------------------------------------------
// `supercharge telemetry on|off|status`
// ---------------------------------------------------------------------------

/// Persist the user's choice from `supercharge telemetry on|off`.
pub fn set_enabled(enabled: bool) -> std::io::Result<()> {
    let path = default_state_path();
    let mut state = load_state_from(&path);
    state.enabled = Some(enabled);
    // Choosing explicitly counts as having seen the notice.
    state.notice_shown = true;
    save_state_to(&path, &state)
}

/// Snapshot for `supercharge telemetry status`.
#[derive(Debug, Clone)]
pub struct Status {
    pub decision: Decision,
    pub anonymous_id: Option<String>,
    pub state_file: PathBuf,
}

pub fn status(config_setting: Option<bool>) -> Status {
    let path = default_state_path();
    let state = load_state_from(&path);
    Status {
        decision: decide(&gather_signals(&state, config_setting)),
        anonymous_id: state.anonymous_id,
        state_file: path,
    }
}

/// What the CLI collects, printed by `supercharge telemetry status`.
pub const WHAT_IS_COLLECTED: &str = "Anonymous usage stats: a random install id, CLI version, OS and CPU \
architecture, install method, which subcommand ran, session starts, the provider / API style / model \
id a session uses, editor (ACP) session starts, and coarse error kinds and exit codes. Never prompts, code, file \
contents, paths, usernames, hostnames, emails, API keys, or model output. The stats service may store the IP address a \
request comes from, but no location is derived from it.";

/// One-time first-run notice.
pub const FIRST_RUN_NOTICE: &str = "Supercharge collects anonymous usage stats (version, OS, commands, \
session counts; never prompts, code, paths, or keys) to improve the CLI.\nTurn it off any time: \
`supercharge telemetry off` or SUPERCHARGE_TELEMETRY=0.";

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// How the process was launched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Interactive terminal UI.
    Tui,
    /// One-shot prompt / headless agent.
    Headless,
    /// Editor agent over stdio (ACP).
    Acp,
    /// Shared background agent process.
    Leader,
    /// Agent server / daemon.
    Server,
    /// Any other subcommand (`models`, `login`, ...).
    Command,
}

impl RunMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tui => "tui",
            Self::Headless => "headless",
            Self::Acp => "acp",
            Self::Leader => "leader",
            Self::Server => "server",
            Self::Command => "command",
        }
    }
}

/// Session start kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    New,
    Load,
    Resume,
}

impl SessionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Load => "load",
            Self::Resume => "resume",
        }
    }
}

/// Wire API family the session's model speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiStyle {
    ChatCompletions,
    Responses,
    Messages,
}

impl ApiStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }
}

/// Coarse error categories. Deliberately no free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// The CLI could not start (config, auth, terminal, ...).
    Startup,
    /// A command returned an error and the process exited non-zero.
    Fatal,
    /// The process panicked.
    Panic,
}

impl ErrorCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup_failure",
            Self::Fatal => "fatal_error",
            Self::Panic => "panic",
        }
    }
}

/// Every event the CLI can send. Fields are enums, static labels, or sanitized ids only.
#[derive(Debug, Clone)]
pub enum UsageEvent {
    AppStarted,
    CommandRun {
        command: &'static str,
    },
    SessionStarted {
        kind: SessionKind,
    },
    ProviderUsed {
        provider: String,
        api_style: ApiStyle,
        model: String,
    },
    AcpSessionStarted {
        kind: SessionKind,
    },
    ErrorOccurred {
        category: ErrorCategory,
        /// Process exit code, when there is one. Never message text.
        code: Option<i32>,
    },
}

impl UsageEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::AppStarted => "app_opened",
            Self::CommandRun { .. } => "command_run",
            Self::SessionStarted { .. } => "session_started",
            Self::ProviderUsed { .. } => "model_selected",
            Self::AcpSessionStarted { .. } => "acp_session_started",
            Self::ErrorOccurred { .. } => "error",
        }
    }

    fn specific_properties(&self, props: &mut Map<String, Value>) {
        match self {
            Self::AppStarted => {}
            Self::CommandRun { command } => {
                props.insert("command".into(), json!(sanitize_label(command)));
            }
            Self::SessionStarted { kind } | Self::AcpSessionStarted { kind } => {
                props.insert("session_kind".into(), json!(kind.as_str()));
            }
            Self::ProviderUsed {
                provider,
                api_style,
                model,
            } => {
                props.insert("provider".into(), json!(sanitize_label(provider)));
                props.insert("api_style".into(), json!(api_style.as_str()));
                props.insert("model".into(), json!(sanitize_model_id(model)));
            }
            Self::ErrorOccurred { category, code } => {
                props.insert("kind".into(), json!(category.as_str()));
                props.insert("code".into(), code.map_or(Value::Null, |c| json!(c)));
            }
        }
    }
}

/// Lowercase `[a-z0-9_ -]` label, max 48 chars; anything else becomes `other`.
pub fn sanitize_label(raw: &str) -> String {
    let trimmed = raw.trim();
    let ok = !trimmed.is_empty()
        && trimmed.len() <= 48
        && trimmed
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | ' '));
    if ok {
        trimmed.to_owned()
    } else {
        "other".to_owned()
    }
}

/// Model ids look like `org/model-name:tag`. Anything path-like, overly long, or
/// carrying unexpected characters is replaced with `custom`.
pub fn sanitize_model_id(raw: &str) -> String {
    let trimmed = raw.trim();
    let ok = !trimmed.is_empty()
        && trimmed.len() <= 96
        && !trimmed.starts_with(['/', '~', '.'])
        && !trimmed.contains("..")
        && !trimmed.contains("//")
        && !trimmed.contains('@')
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':'));
    if ok {
        trimmed.to_owned()
    } else {
        "custom".to_owned()
    }
}

/// Process-wide context attached to every event.
#[derive(Debug, Clone)]
struct BaseContext {
    anonymous_id: String,
    app_version: String,
    os: &'static str,
    arch: &'static str,
    install_method: Option<&'static str>,
    run_mode: RunMode,
    is_child: bool,
}

fn os_label() -> &'static str {
    match std::env::consts::OS {
        "macos" => "Mac OS X",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    }
}

/// Builds a single capture item for the batch endpoint.
fn build_item(base: &BaseContext, event: &UsageEvent, timestamp: String) -> Value {
    let mut props = Map::new();
    props.insert("distinct_id".into(), json!(base.anonymous_id));
    // Common props shared with the desktop app's usage stats.
    props.insert("source".into(), json!("cli"));
    props.insert("app".into(), json!("cli"));
    props.insert("app_version".into(), json!(base.app_version));
    props.insert("platform".into(), json!("terminal"));
    props.insert("os".into(), json!(std::env::consts::OS));
    props.insert("arch".into(), json!(base.arch));
    props.insert("$app_version".into(), json!(base.app_version));
    props.insert("$os".into(), json!(base.os));
    props.insert("run_mode".into(), json!(base.run_mode.as_str()));
    props.insert("child_process".into(), json!(base.is_child));
    if let Some(method) = base.install_method {
        props.insert("install_method".into(), json!(sanitize_label(method)));
    }
    props.insert("$lib".into(), json!("supercharge-cli"));
    // Anonymous events: no person profile, no IP in the payload, no IP-based location.
    // (The ingestion service may still store the request's source IP.)
    props.insert("$process_person_profile".into(), json!(false));
    props.insert("$geoip_disable".into(), json!(true));
    props.insert("$ip".into(), Value::Null);
    event.specific_properties(&mut props);
    for value in props.values_mut() {
        supercharge_secrets::redact_json_string_values(value);
    }
    json!({
        "event": event.name(),
        "distinct_id": base.anonymous_id,
        "properties": Value::Object(props),
        "timestamp": timestamp,
    })
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ---------------------------------------------------------------------------
// Background sender
// ---------------------------------------------------------------------------

enum Msg {
    Event(Value),
    Flush(mpsc::Sender<()>),
}

struct Recorder {
    base: BaseContext,
    tx: Mutex<mpsc::Sender<Msg>>,
}

static RECORDER: OnceLock<Recorder> = OnceLock::new();

/// Options supplied by the binary at startup.
#[derive(Debug, Clone)]
pub struct InitOptions {
    pub app_version: String,
    pub run_mode: RunMode,
    pub install_method: Option<&'static str>,
    /// `[telemetry] usage_stats` from config.toml, when set.
    pub config_setting: Option<bool>,
}

/// Result of [`init`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitOutcome {
    pub decision: Decision,
    /// True for a top-level process; children skip `app_started`/`command_run`.
    pub top_level: bool,
    /// Whether the caller should print [`FIRST_RUN_NOTICE`] (then call [`mark_notice_shown`]).
    pub show_notice: bool,
}

/// Initialize usage stats for this process. Cheap (one small file read, no
/// network). Returns `Decision` so callers can skip work when disabled.
pub fn init(opts: InitOptions) -> InitOutcome {
    let path = default_state_path();
    let mut state = load_state_from(&path);
    let decision = decide(&gather_signals(&state, opts.config_setting));
    let is_child = std::env::var_os(CHILD_MARKER_ENV).is_some();
    if !decision.is_enabled() {
        return InitOutcome {
            decision,
            top_level: !is_child,
            show_notice: false,
        };
    }
    let anonymous_id = ensure_anonymous_id(&path, &mut state);
    let base = BaseContext {
        anonymous_id,
        app_version: opts.app_version,
        os: os_label(),
        arch: std::env::consts::ARCH,
        install_method: opts.install_method,
        run_mode: opts.run_mode,
        is_child,
    };
    let (tx, rx) = mpsc::channel::<Msg>();
    let spawned = std::thread::Builder::new()
        .name("usage-stats".into())
        .spawn(move || worker(rx));
    if spawned.is_err() {
        return InitOutcome {
            decision,
            top_level: !is_child,
            show_notice: false,
        };
    }
    let _ = RECORDER.set(Recorder {
        base,
        tx: Mutex::new(tx),
    });
    if !is_child {
        // Children (leader, helpers) inherit this and stay quiet about startup.
        // SAFETY: called from `main` during single-threaded startup, before the
        // async runtime and before any other thread reads the environment.
        unsafe { std::env::set_var(CHILD_MARKER_ENV, "1") };
    }
    install_panic_hook();
    InitOutcome {
        decision,
        top_level: !is_child,
        show_notice: !is_child && !state.notice_shown,
    }
}

/// Record that the first-run notice was printed.
pub fn mark_notice_shown() {
    let path = default_state_path();
    let mut state = load_state_from(&path);
    state.notice_shown = true;
    let _ = save_state_to(&path, &state);
}

/// Whether usage stats are active in this process.
pub fn is_active() -> bool {
    RECORDER.get().is_some()
}

/// Whether this process is a top-level invocation (not a spawned helper).
pub fn is_top_level() -> bool {
    RECORDER.get().is_some_and(|r| !r.base.is_child)
}

/// The run mode recorded at init, if active.
pub fn run_mode() -> Option<RunMode> {
    RECORDER.get().map(|r| r.base.run_mode)
}

/// Queue an event. Never blocks on the network; no-op when disabled.
pub fn track(event: UsageEvent) {
    let Some(rec) = RECORDER.get() else {
        return;
    };
    let item = build_item(&rec.base, &event, now_rfc3339());
    if let Ok(tx) = rec.tx.lock() {
        let _ = tx.send(Msg::Event(item));
    }
}

/// Send anything queued, waiting at most `timeout`. Call before process exit.
pub fn flush(timeout: Duration) {
    let Some(rec) = RECORDER.get() else {
        return;
    };
    let (ack_tx, ack_rx) = mpsc::channel();
    let sent = rec
        .tx
        .lock()
        .map(|tx| tx.send(Msg::Flush(ack_tx)).is_ok())
        .unwrap_or(false);
    if sent {
        let _ = ack_rx.recv_timeout(timeout);
    }
}

fn install_panic_hook() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    if INSTALLED.set(()).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        track(UsageEvent::ErrorOccurred {
            category: ErrorCategory::Panic,
            code: None,
        });
        flush(Duration::from_millis(700));
        previous(info);
    }));
}

fn worker(rx: mpsc::Receiver<Msg>) {
    let key = project_key();
    let url = format!("{}/batch/", ingestion_host());
    let mut client: Option<reqwest::blocking::Client> = None;
    let mut pending: Vec<Value> = Vec::new();
    let mut first_pending_at: Option<Instant> = None;
    loop {
        let wait = match first_pending_at {
            Some(t) => BATCH_WINDOW.saturating_sub(t.elapsed()),
            None => Duration::from_secs(3600),
        };
        let mut acks: Vec<mpsc::Sender<()>> = Vec::new();
        let mut disconnected = false;
        match rx.recv_timeout(wait) {
            Ok(Msg::Event(item)) => {
                if pending.len() < MAX_QUEUED {
                    pending.push(item);
                }
                first_pending_at.get_or_insert_with(Instant::now);
            }
            Ok(Msg::Flush(ack)) => acks.push(ack),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => disconnected = true,
        }
        let window_elapsed = first_pending_at.is_some_and(|t| t.elapsed() >= BATCH_WINDOW);
        let should_send = !pending.is_empty()
            && (window_elapsed || !acks.is_empty() || disconnected || pending.len() >= MAX_BATCH);
        if should_send {
            let batch = std::mem::take(&mut pending);
            first_pending_at = None;
            send_batch(&mut client, &url, &key, batch);
        }
        for ack in acks {
            let _ = ack.send(());
        }
        if disconnected {
            return;
        }
    }
}

fn send_batch(
    client: &mut Option<reqwest::blocking::Client>,
    url: &str,
    key: &str,
    batch: Vec<Value>,
) {
    if client.is_none() {
        *client = supercharge_extra_ca::build_blocking_reqwest_client(|b| {
            b.timeout(REQUEST_TIMEOUT)
                .connect_timeout(CONNECT_TIMEOUT)
                .user_agent(concat!(
                    "supercharge-usage-stats/",
                    env!("CARGO_PKG_VERSION")
                ))
        })
        .ok();
    }
    let Some(client) = client.as_ref() else {
        return;
    };
    let body = batch_body(key, batch);
    // Failures are silent by design: analytics must never affect the CLI.
    let _ = client.post(url).json(&body).send();
}

fn batch_body(key: &str, batch: Vec<Value>) -> Value {
    json!({ "api_key": key, "historical_migration": false, "batch": batch })
}

#[cfg(test)]
#[path = "usage_stats_tests.rs"]
mod tests;
