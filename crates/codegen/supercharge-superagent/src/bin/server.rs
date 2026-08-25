use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    convert::Infallible,
    env,
    net::SocketAddr,
    path::{Path as FsPath, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::{Mutex, broadcast},
};
use tracing::Instrument;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message as TungsteniteMessage};

use base64::Engine as _;

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State, WebSocketUpgrade, ws::Message},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use supercharge_superagent::{
    ApprovalPolicy, ScheduledJob, SuperagentConfig,
    browser::{self, StreamStatus},
    controls::{
        handle_text as handle_control_text, init_global as init_controls,
        is_emergency_stopped as control_is_emergency_stopped, is_stopped as control_is_stopped,
        set_emergency_stopped,
    },
    delivery::{DeliveryTarget, dispatch_text},
    discord::DiscordClient,
    email::{EmailClient, EmailConfig},
    gateway::GatewayStateStore,
    integrations::{
        SkillRegistry, discover_http_tools, discover_stdio_tools, invoke_http_tool,
        invoke_stdio_tool, parse_mcp_servers,
    },
    jobs::{AgentJob, JobStatus, JobStore},
    live::{
        self, LiveEvent, app_events_for, e2b_agent_rules_for, next_messenger_progress,
        parse_cli_stream_line,
    },
    matrix::{MatrixClient, MatrixConfig},
    media::extract_media,
    memory::{MemoryEntry, MemoryStore},
    provider::{
        ProviderConfigRequest, ProviderProfile, builtin_profiles, configured_profile, health_check,
        validate_request, write_runtime_config,
    },
    redaction::redact_secrets,
    schedule_store::ScheduleStore,
    sharing::rewrite_media_links,
    signal::{SignalClient, SignalConfig},
    slack::{SlackClient, SlackConfig},
    subagents::{
        SubagentError, SubagentRequest, global as global_subagents, init_global as init_subagents,
    },
    telegram::{TelegramClient, TelegramConfig, TelegramUpdate},
    webhook::{WebhookClient, WebhookConfig},
    whatsapp::{WhatsAppClient, WhatsAppConfig},
};
#[cfg(feature = "e2b")]
use supercharge_superagent::{E2bExecutor, PersistentExecutionRequest, SandboxPlan};
use tower_http::limit::RequestBodyLimitLayer;

#[derive(Clone)]
struct RateLimiter {
    per_minute: u32,
    buckets: HashMap<String, RateBucket>,
}

#[derive(Clone)]
struct RateBucket {
    window_started: u64,
    requests: u32,
}

impl RateLimiter {
    fn from_env() -> Self {
        let per_minute = env::var("SUPERAGENT_RATE_LIMIT_PER_MINUTE")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        Self {
            per_minute,
            buckets: HashMap::new(),
        }
    }

    fn allow(&mut self, key: &str, now: u64) -> bool {
        if self.per_minute == 0 {
            return true;
        }
        let bucket = self.buckets.entry(key.to_owned()).or_insert(RateBucket {
            window_started: now,
            requests: 0,
        });
        if now.saturating_sub(bucket.window_started) >= 60 {
            bucket.window_started = now;
            bucket.requests = 0;
        }
        if bucket.requests >= self.per_minute {
            return false;
        }
        bucket.requests += 1;
        true
    }
}

#[derive(Clone)]
struct AppState {
    config: SuperagentConfig,
    started_at: u64,
    approval: ApprovalPolicy,
    rate_limiter: Arc<Mutex<RateLimiter>>,
    api_token: Option<String>,
    #[cfg(feature = "e2b")]
    sandbox_plan: SandboxPlan,
    whatsapp: Option<WhatsAppClient>,
    webhook: Option<(WebhookClient, WebhookConfig)>,
    job_store: Arc<Mutex<JobStore>>,
    memory_store: Arc<Mutex<MemoryStore>>,
    schedule_store: Arc<Mutex<ScheduleStore>>,
    app_events: broadcast::Sender<Value>,
    app_event_log: Arc<StdMutex<VecDeque<Value>>>,
    app_event_seq: Arc<AtomicU64>,
    sessions: Arc<Mutex<BTreeMap<String, DaemonSession>>>,
    active_app_session: Arc<Mutex<String>>,
    session_state_path: String,
    metrics: Arc<RuntimeMetrics>,
    #[cfg(feature = "e2b")]
    e2b_sessions: Arc<Mutex<E2bSessionStore>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DaemonSession {
    id: String,
    created_at: u64,
    updated_at: u64,
    #[serde(default)]
    title: String,
    #[serde(default = "default_session_status")]
    status: String,
    #[serde(default)]
    messages: Vec<ConversationMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConversationMessage {
    role: String,
    content: String,
}

fn default_session_status() -> String {
    "active".to_owned()
}

fn load_daemon_sessions(path: &str) -> anyhow::Result<BTreeMap<String, DaemonSession>> {
    match std::fs::read_to_string(path) {
        Ok(content) if !content.trim().is_empty() => Ok(serde_json::from_str(&content)?),
        Ok(_) => Ok(BTreeMap::new()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error.into()),
    }
}

fn persist_daemon_sessions(
    path: &str,
    sessions: &BTreeMap<String, DaemonSession>,
) -> anyhow::Result<()> {
    let path = std::path::Path::new(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(sessions)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(temporary, path)?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
struct CreateSessionRequest {
    id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct ProviderUsage {
    attempts: u64,
    succeeded: u64,
    failed: u64,
    prompt_chars: u64,
    reply_chars: u64,
    total_duration_ms: u64,
    provider_usage: BTreeMap<String, ProviderUsage>,
}

#[derive(Default)]
struct RuntimeMetrics {
    turns_started: AtomicU64,
    turns_succeeded: AtomicU64,
    turns_failed: AtomicU64,
    prompt_chars: AtomicU64,
    reply_chars: AtomicU64,
    total_duration_ms: AtomicU64,
    provider_usage: StdMutex<BTreeMap<String, ProviderUsage>>,
}

static RUNTIME_METRICS: OnceLock<Arc<RuntimeMetrics>> = OnceLock::new();

fn runtime_metrics() -> Arc<RuntimeMetrics> {
    RUNTIME_METRICS
        .get_or_init(|| Arc::new(RuntimeMetrics::default()))
        .clone()
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    config_fingerprint: String,
}

#[derive(Debug, Deserialize)]
struct ApprovalQuery {
    command: String,
    #[serde(default)]
    approved: bool,
}

#[derive(Debug, Serialize)]
struct ApprovalResponse {
    allowed: bool,
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExecuteRequest {
    command: String,
    #[serde(default)]
    approved: bool,
}

#[cfg(feature = "e2b")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct E2bSessionRecord {
    session_id: String,
    sandbox_id: String,
    strategy: String,
    cwd: Option<String>,
    workspace_root: String,
    created_at: u64,
    updated_at: u64,
}

#[cfg(feature = "e2b")]
#[derive(Debug, Default)]
struct E2bSessionStore {
    path: String,
    sessions: std::collections::BTreeMap<String, E2bSessionRecord>,
}

#[cfg(feature = "e2b")]
impl E2bSessionStore {
    fn load(path: String) -> anyhow::Result<Self> {
        let sessions = match std::fs::read_to_string(&path) {
            Ok(content) if !content.trim().is_empty() => serde_json::from_str(&content)?,
            Ok(_) | Err(_) => std::collections::BTreeMap::new(),
        };
        Ok(Self { path, sessions })
    }

    fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = std::path::Path::new(&self.path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(&self.sessions)?;
        let temporary = format!("{}.tmp", self.path);
        std::fs::write(&temporary, content)?;
        std::fs::rename(temporary, &self.path)?;
        Ok(())
    }
}

#[cfg(feature = "e2b")]
#[derive(Debug, Deserialize)]
struct E2bSessionRequest {
    session_id: Option<String>,
    sandbox_id: Option<String>,
    strategy: Option<String>,
    cwd: Option<String>,
    #[serde(default = "default_e2b_workspace_root")]
    workspace_root: String,
}

#[cfg(feature = "e2b")]
fn default_e2b_workspace_root() -> String {
    env::var("SUPERAGENT_E2B_WORKSPACE_ROOT").unwrap_or_else(|_| "/home/user/workspace".to_owned())
}

#[cfg(feature = "e2b")]
#[derive(Debug, Deserialize)]
struct E2bExecRequest {
    command: String,
    cwd: Option<String>,
    timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct MemoryRequest {
    session_key: String,
    content: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default = "default_memory_kind")]
    kind: String,
    #[serde(default = "default_memory_importance")]
    importance: u8,
}

fn default_memory_kind() -> String {
    "note".to_owned()
}

fn default_memory_importance() -> u8 {
    50
}

#[derive(Debug, Deserialize)]
struct MemoryCompactRequest {
    #[serde(default = "default_memory_capacity")]
    max_entries: usize,
}

#[derive(Debug, Deserialize)]
struct MemoryRestoreRequest {
    entries: Vec<MemoryEntry>,
    #[serde(default)]
    replace: bool,
}

#[derive(Debug, Deserialize)]
struct ScheduleRequest {
    name: String,
    prompt: String,
    interval_seconds: u64,
    #[serde(default)]
    cron: Option<String>,
    home_channel: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScheduleToggleRequest {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct MediaParseRequest {
    response: String,
}

fn default_memory_capacity() -> usize {
    10_000
}

#[derive(Debug, Deserialize)]
struct McpToolsQuery {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct McpToolCallRequest {
    server: String,
    tool: String,
    #[serde(default)]
    arguments: Value,
    timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SkillRunRequest {
    session_key: String,
    prompt: String,
}

#[derive(Debug, Deserialize)]
struct MemoryQuery {
    session_key: Option<String>,
    #[serde(default)]
    q: String,
    #[serde(default = "default_memory_limit")]
    limit: usize,
}

fn default_memory_limit() -> usize {
    20
}

#[derive(Debug, Deserialize)]
struct EnqueueRequest {
    session_key: String,
    platform: String,
    chat_id: i64,
    prompt: String,
    #[serde(default = "default_attempts")]
    max_attempts: u32,
}

fn default_attempts() -> u32 {
    3
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug, Serialize)]
struct DaemonStatus {
    daemon: &'static str,
    gateways: Vec<String>,
    queued_jobs: usize,
    running_jobs: usize,
    succeeded_jobs: usize,
    failed_jobs: usize,
}

#[derive(Debug, Serialize)]
struct ObservabilityResponse {
    uptime_seconds: u64,
    active_gateways: Vec<String>,
    queued_jobs: usize,
    running_jobs: usize,
    succeeded_jobs: usize,
    failed_jobs: usize,
    memory_entries: usize,
    memory_sessions: usize,
    schedules: usize,
    enabled_schedules: usize,
    turns_started: u64,
    turns_succeeded: u64,
    turns_failed: u64,
    prompt_chars: u64,
    reply_chars: u64,
    total_duration_ms: u64,
    provider_usage: BTreeMap<String, ProviderUsage>,
}

#[derive(Debug, Serialize)]
struct ConnectionReadiness {
    name: &'static str,
    mode: &'static str,
    configured: bool,
    missing_env: Vec<&'static str>,
}

fn connection_readiness(
    name: &'static str,
    mode: &'static str,
    required_env: &[&'static str],
) -> ConnectionReadiness {
    let missing_env = required_env
        .iter()
        .copied()
        .filter(|key| {
            env::var(key)
                .ok()
                .is_none_or(|value| value.trim().is_empty())
        })
        .collect::<Vec<_>>();
    ConnectionReadiness {
        name,
        mode,
        configured: missing_env.is_empty(),
        missing_env,
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = env::var("SUPERAGENT_LOG_FILTER").unwrap_or_else(|_| "info".to_owned());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
    let bind = env::var("SUPERAGENT_BIND").unwrap_or_else(|_| "127.0.0.1:8787".to_owned());
    let addr: SocketAddr = bind.parse()?;
    let api_token = env::var("SUPERAGENT_API_TOKEN")
        .or_else(|_| env::var("SUPERAGENT_GATEWAY_TOKEN"))
        .or_else(|_| env::var("SUPERAGENT_APP_INBOX_SECRET"))
        .ok()
        .filter(|v| !v.is_empty());
    let webhook = env::var("GENERIC_WEBHOOK_SECRET").ok().map(|secret| {
        (
            WebhookClient::new(),
            WebhookConfig {
                secret: Some(secret),
                allowed_sources: Default::default(),
            },
        )
    });
    let whatsapp = if env::var("WHATSAPP_ACCESS_TOKEN").is_ok() {
        Some(WhatsAppClient::new(WhatsAppConfig::from_env()?))
    } else {
        None
    };
    let control_state_path = env::var("SUPERAGENT_CONTROL_STATE")
        .unwrap_or_else(|_| "./runtime/controls.json".to_owned());
    init_controls(control_state_path)?;
    let provider_profile = configured_profile();
    let provider_config_path = env::var("SUPERAGENT_PROVIDER_CONFIG_PATH")
        .or_else(|_| env::var("SUPERCHARGE_CONFIG_PATH"))
        .unwrap_or_else(|_| "./runtime/provider-config.toml".to_owned());
    if let Err(error) = write_runtime_config(&provider_config_path, &provider_profile) {
        eprintln!("provider profile initialization failed: {error}");
    }
    if let Some(binary) = configured_agent_binary() {
        let max_depth = env::var("SUPERAGENT_MAX_SUBAGENT_DEPTH")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(3);
        let max_parallel = env::var("SUPERAGENT_MAX_PARALLEL_SUBAGENTS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(4);
        init_subagents(
            binary.to_string_lossy().into_owned(),
            max_depth,
            max_parallel,
        );
    }
    let memory_state_path =
        env::var("SUPERAGENT_MEMORY_STATE").unwrap_or_else(|_| "./runtime/memory.json".to_owned());
    let memory_store = Arc::new(Mutex::new(MemoryStore::load(memory_state_path)?));
    let job_state_path =
        env::var("SUPERAGENT_JOB_STATE").unwrap_or_else(|_| "./runtime/jobs.json".to_owned());
    let job_store = Arc::new(Mutex::new(JobStore::load(job_state_path)?));
    let schedule_state_path = env::var("SUPERAGENT_SCHEDULE_STATE")
        .unwrap_or_else(|_| "./runtime/schedules.json".to_owned());
    let schedule_store = Arc::new(Mutex::new(ScheduleStore::load(schedule_state_path)?));
    let (app_events, _) = broadcast::channel(256);
    let app_event_log = Arc::new(StdMutex::new(VecDeque::with_capacity(APP_EVENT_LOG_LIMIT)));
    let session_state_path = env::var("SUPERAGENT_SESSION_STATE")
        .unwrap_or_else(|_| "./runtime/sessions.json".to_owned());
    let sessions = load_daemon_sessions(&session_state_path)?;
    let active_app_session = if sessions.contains_key("owner") {
        "owner".to_owned()
    } else {
        sessions
            .keys()
            .next_back()
            .cloned()
            .unwrap_or_else(|| "owner".to_owned())
    };
    let state = AppState {
        config: SuperagentConfig::default(),
        started_at: unix_seconds(),
        approval: ApprovalPolicy::default(),
        rate_limiter: Arc::new(Mutex::new(RateLimiter::from_env())),
        api_token,
        #[cfg(feature = "e2b")]
        sandbox_plan: SandboxPlan::default(),
        whatsapp,
        webhook,
        job_store,
        memory_store,
        schedule_store,
        app_events,
        app_event_log,
        app_event_seq: Arc::new(AtomicU64::new(0)),
        sessions: Arc::new(Mutex::new(sessions)),
        active_app_session: Arc::new(Mutex::new(active_app_session)),
        session_state_path,
        metrics: runtime_metrics(),
        #[cfg(feature = "e2b")]
        e2b_sessions: Arc::new(Mutex::new(E2bSessionStore::load(
            env::var("SUPERAGENT_E2B_SESSION_STATE")
                .unwrap_or_else(|_| "./runtime/e2b-sessions.json".to_owned()),
        )?)),
    };

    let scheduler_state = Arc::new(state.clone());
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let due = {
                let mut schedules = scheduler_state.schedule_store.lock().await;
                match schedules.claim_due(chrono::Utc::now()) {
                    Ok(due) => due,
                    Err(error) => {
                        eprintln!("schedule tick failed: {error}");
                        Vec::new()
                    }
                }
            };
            for schedule in due {
                let target = schedule
                    .home_channel
                    .as_deref()
                    .and_then(|value| value.parse::<DeliveryTarget>().ok());
                let (platform, chat_id) = target
                    .as_ref()
                    .map(|value| (format!("target:{}", value.session_channel()), 0))
                    .unwrap_or_else(|| ("scheduler".to_owned(), 0));
                let session_key = format!("schedule:{}", schedule.id);
                let mut jobs = scheduler_state.job_store.lock().await;
                if let Err(error) = jobs.enqueue(session_key, platform, chat_id, schedule.prompt, 3)
                {
                    eprintln!("scheduled job enqueue failed: {error}");
                }
            }
        }
    });

    let worker_state = Arc::new(state.clone());
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let job = {
                let mut jobs = worker_state.job_store.lock().await;
                let now = unix_seconds();
                if let Err(error) = jobs.recover_expired_leases(now) {
                    eprintln!("job lease recovery failed: {error}");
                }
                match jobs.claim_next(now, 300) {
                    Ok(job) => job,
                    Err(error) => {
                        eprintln!("job claim failed: {error}");
                        None
                    }
                }
            };
            let Some(job) = job else {
                continue;
            };
            let outcome = run_supercharge_prompt(&job.session_key, &job.prompt).await;
            let mut jobs = worker_state.job_store.lock().await;
            match outcome {
                Ok(reply) => {
                    if let Some(reply_text) = reply.as_deref()
                        && let Some(target) = job.platform.strip_prefix("target:")
                    {
                        match target.parse::<DeliveryTarget>() {
                            Ok(target) => {
                                let prepared = prepare_reply_for_delivery(reply_text).await;
                                if let Err(error) = dispatch_text(&target, &prepared).await {
                                    eprintln!("job result delivery failed: {error}");
                                }
                            }
                            Err(error) => eprintln!("job target parsing failed: {error}"),
                        }
                    }
                    if let Err(error) = jobs.complete_with_result(&job.id, reply) {
                        eprintln!("job completion failed: {error}");
                    }
                }
                Err(error) => {
                    if let Err(store_error) = jobs.fail(&job.id, error.to_string(), 15) {
                        eprintln!("job failure persistence failed: {store_error}");
                    }
                }
            }
        }
    });

    let protected = Router::new()
        .route("/v1/approval/check", get(check_approval))
        .route("/v1/exec", post(execute))
        .route("/v1/jobs", get(list_jobs).post(enqueue_job))
        .route("/v1/jobs/{id}", get(get_job))
        .route("/v1/schedules", get(list_schedules).post(create_schedule))
        .route("/v1/schedules/{id}", delete(delete_schedule))
        .route("/v1/schedules/{id}/enabled", post(toggle_schedule))
        .route("/v1/daemon/status", get(daemon_status))
        .route("/v1/emergency-stop", post(emergency_stop))
        .route("/v1/emergency-resume", post(emergency_resume))
        .route("/v1/observability", get(observability))
        .route("/v1/computer/status", get(browser_status))
        .route("/v1/computer/browser", get(browser_stream))
        .route("/v1/computer/browser/vnc", get(browser_vnc))
        .route("/v1/computer/control", post(browser_control))
        .route("/v1/computer/handoff", post(browser_handoff))
        .route("/v1/status", get(app_status))
        .route("/v1/commands", get(app_commands))
        .route(
            "/v1/conversations",
            get(list_app_conversations).post(create_app_conversation),
        )
        .route("/v1/conversations/new", post(create_app_conversation))
        .route("/v1/conversations/switch", post(switch_app_conversation))
        .route("/v1/conversations/{session_id}", get(get_app_conversation))
        .route("/v1/chat", post(app_chat))
        .route("/v1/chat/events", get(app_chat_events))
        .route("/v1/chat/stop", post(app_chat_stop))
        .route("/v1/chat/approve", post(app_chat_approve))
        .route("/v1/chat/answer", post(app_chat_answer))
        .route("/v1/chat/pick", post(app_chat_pick))
        .route("/api/v1/chat", post(app_chat))
        .route("/api/v1/sessions", get(list_sessions).post(create_session))
        .route("/api/v1/stream", get(api_stream))
        .route("/v1/connections", get(connection_status))
        .route(
            "/v1/providers",
            get(list_providers).post(configure_provider),
        )
        .route("/v1/providers/health", get(provider_health))
        .route("/v1/memory", get(search_memory).post(remember_memory))
        .route("/v1/memory/stats", get(memory_stats))
        .route("/v1/memory/export", get(export_memory))
        .route("/v1/memory/restore", post(restore_memory))
        .route("/v1/memory/compact", post(compact_memory))
        .route("/v1/memory/{id}", delete(delete_memory))
        .route("/v1/media/parse", post(parse_media))
        .route("/v1/integrations/skills", get(list_skills))
        .route("/v1/integrations/skills/{name}", get(get_skill))
        .route("/v1/integrations/skills/{name}/run", post(run_skill))
        .route("/v1/integrations/mcp", get(list_mcp_servers))
        .route("/v1/integrations/mcp/tools", get(list_mcp_tools))
        .route("/v1/integrations/mcp/call", post(call_mcp_tool))
        .route("/v1/subagents", post(run_subagent));
    #[cfg(feature = "e2b")]
    let protected = protected
        .route(
            "/v1/e2b/sessions",
            get(list_e2b_sessions).post(create_e2b_session),
        )
        .route(
            "/v1/e2b/sessions/{id}",
            get(get_e2b_session).delete(delete_e2b_session),
        )
        .route("/v1/e2b/sessions/{id}/exec", post(execute_e2b_session))
        .route("/v1/e2b/sessions/{id}/wake", post(wake_e2b_session))
        .route("/v1/e2b/sessions/{id}/pause", post(pause_e2b_session));
    let protected = protected
        .layer(RequestBodyLimitLayer::new(25 * 1024 * 1024))
        .layer(middleware::from_fn(add_security_headers))
        .with_state(Arc::new(state.clone()))
        .layer(middleware::from_fn_with_state(
            Arc::new(state.clone()),
            auth,
        ));

    let app = Router::new()
        .route("/healthz", get(healthz))
        // Vectra app inbox probes GET /health (XibeCode-compatible).
        .route("/health", get(healthz))
        .route("/v1/capabilities", get(capabilities))
        .route(
            "/webhooks/whatsapp",
            get(whatsapp_verify).post(whatsapp_webhook),
        )
        .route("/webhooks/telegram", post(telegram_webhook))
        .route("/webhooks/generic", post(generic_webhook))
        .with_state(Arc::new(state.clone()))
        .merge(protected);

    if env::var("MATRIX_ACCESS_TOKEN").is_ok() {
        tokio::spawn(async {
            if let Err(error) = run_matrix_daemon().await {
                eprintln!("matrix gateway stopped: {error}");
            }
        });
    }
    if env::var("SIGNAL_ACCOUNT").is_ok() {
        tokio::spawn(async {
            if let Err(error) = run_signal_daemon().await {
                eprintln!("signal gateway stopped: {error}");
            }
        });
    }
    if env::var("SLACK_APP_TOKEN").is_ok() {
        tokio::spawn(async {
            if let Err(error) = run_slack_daemon().await {
                eprintln!("slack gateway stopped: {error}");
            }
        });
    }
    if env::var("DISCORD_BOT_TOKEN").is_ok() {
        tokio::spawn(async {
            if let Err(error) = run_discord_daemon().await {
                eprintln!("discord gateway stopped: {error}");
            }
        });
    }
    if env::var("EMAIL_USERNAME").is_ok() && env::var("EMAIL_PASSWORD").is_ok() {
        tokio::spawn(async {
            match EmailConfig::from_env() {
                Ok(config) => {
                    if let Err(error) = run_email_daemon(config).await {
                        eprintln!("email gateway stopped: {error}");
                    }
                }
                Err(error) => eprintln!("email gateway configuration failed: {error}"),
            }
        });
    }
    if env::var("TELEGRAM_BOT_TOKEN").is_ok() {
        if env::var("TELEGRAM_WEBHOOK_URL").is_ok() {
            tokio::spawn(async {
                if let Err(error) = configure_telegram_webhook().await {
                    eprintln!("telegram webhook setup failed: {error}");
                }
            });
        } else {
            tokio::spawn(async {
                if let Err(error) = run_telegram_daemon().await {
                    eprintln!("telegram gateway stopped: {error}");
                }
            });
        }
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("superagent server listening on {addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn run_email_daemon(
    config: EmailConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = EmailClient::new();
    loop {
        match client.fetch_unseen(&config).await {
            Ok(messages) => {
                for message in messages {
                    match run_supercharge_prompt(&message.session_key(), &message.text).await {
                        Ok(Some(reply)) if !reply.trim().is_empty() => {
                            let subject = if message.subject.starts_with("Re:") {
                                message.subject.clone()
                            } else {
                                format!("Re: {}", message.subject)
                            };
                            if let Err(error) = client
                                .send_text(&config, &message.sender, &subject, &reply)
                                .await
                            {
                                eprintln!("email reply delivery failed: {error}");
                            }
                        }
                        Ok(_) => {}
                        Err(error) => eprintln!("email agent turn failed: {error}"),
                    }
                }
            }
            Err(error) => eprintln!("email receive failed; retrying: {error}"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(config.poll_seconds)).await;
    }
}

async fn configure_telegram_webhook() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = TelegramConfig::from_env()?;
    let webhook_url = env::var("TELEGRAM_WEBHOOK_URL")?;
    let secret = config
        .webhook_secret
        .clone()
        .ok_or("TELEGRAM_WEBHOOK_SECRET must be configured when TELEGRAM_WEBHOOK_URL is set")?;
    let client = TelegramClient::new(config);
    client.set_webhook(&webhook_url, Some(&secret)).await?;
    eprintln!("telegram webhook configured at {webhook_url}");
    Ok(())
}

async fn run_telegram_daemon() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = TelegramConfig::from_env()?;
    let state_path = env::var("SUPERAGENT_GATEWAY_STATE")
        .unwrap_or_else(|_| "./runtime/telegram-gateway.json".to_owned());
    let mut state = GatewayStateStore::load(state_path)?;
    let echo_mode = env::var("TELEGRAM_ECHO_MODE")
        .ok()
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
    let client = TelegramClient::new(config);

    loop {
        match client.poll_once(&mut state).await {
            Ok(messages) => {
                for message in messages {
                    eprintln!(
                        "telegram message: chat={} user={:?} session={}",
                        message.chat_id,
                        message.user_id,
                        message.session_key().as_str()
                    );
                    if echo_mode {
                        send_chunked(
                            &client,
                            message.chat_id,
                            &format!("superagent received: {}", message.text),
                        )
                        .await?;
                    } else if configured_agent_binary().is_some() {
                        let chat_id = message.chat_id;
                        let progress_id = client
                            .send_message_with_id(chat_id, "Working…")
                            .await
                            .ok();
                        let progress_client = client.clone();
                        let on_progress: Arc<dyn Fn(String) + Send + Sync> =
                            Arc::new(move |text: String| {
                                if let Some(message_id) = progress_id {
                                    let client = progress_client.clone();
                                    tokio::spawn(async move {
                                        let _ = client.edit_message(chat_id, message_id, &text).await;
                                    });
                                }
                            });
                        match run_supercharge_turn_live(&message, on_progress).await {
                            Ok(Some(reply)) if !reply.is_empty() => {
                                let prepared = prepare_reply_for_delivery(&reply).await;
                                if let Some(message_id) = progress_id {
                                    let _ = client
                                        .edit_message(chat_id, message_id, &prepared)
                                        .await;
                                } else {
                                    send_chunked(&client, chat_id, &prepared).await?;
                                }
                            }
                            Ok(_) => {}
                            Err(error) => {
                                eprintln!("agent headless turn failed: {error}");
                                client
                                    .send_message(
                                        message.chat_id,
                                        "The agent turn failed; check the daemon logs.",
                                    )
                                    .await?;
                            }
                        }
                    }
                }
            }
            Err(error) => {
                eprintln!("telegram poll failed; retrying: {error}");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn run_signal_daemon() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = SignalClient::new(SignalConfig::from_env()?);
    loop {
        match client.receive_once().await {
            Ok(messages) => {
                for message in messages {
                    eprintln!(
                        "signal message: sender={} session={}",
                        message.sender,
                        message.session_key()
                    );
                    if configured_agent_binary().is_some()
                        && let Ok(Some(reply)) =
                            run_supercharge_prompt(&message.session_key(), &message.text).await
                    {
                        client.send_message(&message.sender, &reply).await?;
                    }
                }
            }
            Err(error) => {
                eprintln!("signal receive failed; retrying: {error}");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn run_matrix_daemon() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = MatrixClient::new(MatrixConfig::from_env()?);
    let state_path =
        env::var("MATRIX_SYNC_STATE").unwrap_or_else(|_| "./runtime/matrix-sync.json".to_owned());
    let mut since: Option<String> = if let Ok(bytes) = tokio::fs::read(&state_path).await {
        serde_json::from_slice(&bytes).ok()
    } else {
        None
    };
    loop {
        match client.sync_once(since.as_deref()).await {
            Ok((next_batch, messages)) => {
                if let Some(next_batch) = next_batch {
                    if let Some(parent) = std::path::Path::new(&state_path).parent() {
                        let _ = tokio::fs::create_dir_all(parent).await;
                    }
                    let _ = tokio::fs::write(&state_path, serde_json::to_vec(&next_batch)?).await;
                    since = Some(next_batch);
                }
                for message in messages {
                    eprintln!(
                        "matrix message: room={} sender={} session={}",
                        message.room_id,
                        message.sender,
                        message.session_key()
                    );
                    if configured_agent_binary().is_some()
                        && let Ok(Some(reply)) =
                            run_supercharge_prompt(&message.session_key(), &message.text).await
                    {
                        let prepared = prepare_reply_for_delivery(&reply).await;
                        client.send_message(&message.room_id, &prepared).await?;
                    }
                }
            }
            Err(error) => {
                eprintln!("matrix sync failed; retrying: {error}");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn run_slack_daemon() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = SlackClient::new(SlackConfig::from_env()?);
    loop {
        match client.run_once().await {
            Ok(messages) => {
                for message in messages {
                    eprintln!(
                        "slack message: channel={} user={} session={}",
                        message.channel_id,
                        message.user_id,
                        message.session_key()
                    );
                    if configured_agent_binary().is_some()
                        && let Ok(Some(reply)) =
                            run_supercharge_prompt(&message.session_key(), &message.text).await
                    {
                        let prepared = prepare_reply_for_delivery(&reply).await;
                        client
                            .send_message(
                                &message.channel_id,
                                &prepared,
                                message.thread_ts.as_deref(),
                            )
                            .await?;
                    }
                }
            }
            Err(error) => {
                eprintln!("slack Socket Mode failed; reconnecting: {error}");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn run_discord_daemon() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = supercharge_superagent::discord::DiscordConfig::from_env()?;
    let client = DiscordClient::new(config);
    loop {
        match client.run_once().await {
            Ok(messages) => {
                for message in messages {
                    eprintln!(
                        "discord message: channel={} user={:?} session={}",
                        message.chat_id,
                        message.user_id,
                        message.session_key().as_str()
                    );
                    if let Some(binary) = configured_agent_binary() {
                        match run_supercharge_turn(&message).await {
                            Ok(Some(reply)) if !reply.is_empty() => {
                                let prepared = prepare_reply_for_delivery(&reply).await;
                                send_discord_chunked(&client, message.chat_id, &prepared).await?;
                            }
                            Ok(_) => {}
                            Err(error) => {
                                eprintln!("agent headless Discord turn failed: {error}");
                                let _ = binary;
                                let _ = client
                                    .send_message(
                                        &message.chat_id.to_string(),
                                        "The agent turn failed; check the daemon logs.",
                                    )
                                    .await;
                            }
                        }
                    }
                }
            }
            Err(error) => {
                eprintln!("discord gateway poll failed; reconnecting: {error}");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn send_discord_chunked(
    client: &DiscordClient,
    channel_id: i64,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let chars: Vec<char> = text.chars().collect();
    for chunk in chars.chunks(1900) {
        client
            .send_message(&channel_id.to_string(), &chunk.iter().collect::<String>())
            .await?;
    }
    Ok(())
}

async fn run_supercharge_turn_live(
    message: &supercharge_superagent::gateway::GatewayMessage,
    on_progress: Arc<dyn Fn(String) + Send + Sync>,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    run_supercharge_prompt_inner(
        message.session_key().as_str(),
        &message.text,
        Some(PromptStreamSink::MessengerProgress(Box::new(move |text| {
            on_progress(text.to_owned());
        }))),
    )
    .await
}

async fn run_supercharge_turn(
    message: &supercharge_superagent::gateway::GatewayMessage,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    run_supercharge_prompt(message.session_key().as_str(), &message.text).await
}

async fn prepare_reply_for_delivery(reply: &str) -> String {
    let root = env::var("SUPERAGENT_SHARE_WORKSPACE_ROOT")
        .or_else(|_| env::var("SUPERAGENT_DAEMON_WORKDIR"))
        .unwrap_or_else(|_| "/home/user/workspace".to_owned());
    let max_bytes = env::var("SUPERAGENT_MAX_MEDIA_BYTES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(25 * 1024 * 1024);
    match rewrite_media_links(reply, root, max_bytes).await {
        Ok(text) => text,
        Err(error) => format!("{reply}\n\nFile sharing failed: {error}"),
    }
}

fn provider_profiles_for_turn() -> Vec<ProviderProfile> {
    let primary = configured_profile();
    let mut profiles = vec![primary.clone()];
    let fallbacks = env::var("SUPERAGENT_PROVIDER_FALLBACKS").unwrap_or_default();
    for id in fallbacks
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if profiles.len() >= 4 || profiles.iter().any(|profile| profile.id == id) {
            continue;
        }
        if let Some(profile) = builtin_profiles()
            .into_iter()
            .find(|profile| profile.id.eq_ignore_ascii_case(id))
        {
            profiles.push(profile);
        }
    }
    profiles
}

fn record_provider_usage(
    metrics: &RuntimeMetrics,
    provider: &str,
    prompt_chars: u64,
    reply_chars: u64,
    duration_ms: u64,
    succeeded: bool,
) {
    if let Ok(mut usage) = metrics.provider_usage.lock() {
        let entry = usage.entry(provider.to_owned()).or_default();
        entry.attempts = entry.attempts.saturating_add(1);
        entry.prompt_chars = entry.prompt_chars.saturating_add(prompt_chars);
        entry.reply_chars = entry.reply_chars.saturating_add(reply_chars);
        entry.total_duration_ms = entry.total_duration_ms.saturating_add(duration_ms);
        if succeeded {
            entry.succeeded = entry.succeeded.saturating_add(1);
        } else {
            entry.failed = entry.failed.saturating_add(1);
        }
    }
}

#[derive(Default)]
struct CliStreamState {
    live_text: String,
    live_thought: String,
    tool_count: u32,
}

enum PromptStreamSink {
    App {
        state: Arc<AppState>,
        chat_id: String,
    },
    MessengerProgress(Box<dyn Fn(&str) + Send + Sync>),
}

fn turn_rules_for(session_key: &str) -> Option<String> {
    if live::sandbox_id().is_some() {
        Some(e2b_agent_rules_for(session_key))
    } else {
        None
    }
}

fn ingest_cli_stream_line(line: &Value, stream: &mut CliStreamState, sink: Option<&PromptStreamSink>) {
    if line.get("type").and_then(Value::as_str) == Some("error") {
        return;
    }
    let events = parse_cli_stream_line(line);
    for event in &events {
        if let LiveEvent::TextDelta(chunk) = event {
            stream.live_text.push_str(chunk);
        }
    }
    match sink {
        Some(PromptStreamSink::App { state, chat_id }) => {
            for event in events {
                match &event {
                    LiveEvent::Thought(chunk) => stream.live_thought.push_str(&chunk),
                    LiveEvent::ToolStart { .. } => {
                        stream.tool_count = stream.tool_count.saturating_add(1);
                    }
                    _ => {}
                }
                for payload in app_events_for(
                    &event,
                    stream.tool_count,
                    &stream.live_text,
                    &stream.live_thought,
                ) {
                    app_emit(state, chat_id, payload);
                }
            }
        }
        Some(PromptStreamSink::MessengerProgress(callback)) => {
            if let Some(text) =
                next_messenger_progress(line, &mut stream.tool_count, &mut stream.live_thought)
            {
                callback(&text);
            }
        }
        None => {
            for event in events {
                match event {
                    LiveEvent::Thought(chunk) => stream.live_thought.push_str(&chunk),
                    LiveEvent::ToolStart { .. } => {
                        stream.tool_count = stream.tool_count.saturating_add(1);
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn read_cli_stream(
    stdout: tokio::process::ChildStdout,
    sink: Option<&PromptStreamSink>,
) -> Result<CliStreamState, String> {
    let mut reader = BufReader::new(stdout).lines();
    let mut stream = CliStreamState::default();
    while let Some(line) = reader
        .next_line()
        .await
        .map_err(|error| redact_secrets(&error.to_string()))?
    {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            if sink.is_none() {
                stream.live_text.push_str(trimmed);
                stream.live_text.push('\n');
            }
            continue;
        };
        ingest_cli_stream_line(&value, &mut stream, sink);
    }
    Ok(stream)
}

async fn execute_headless_cli_turn(
    binary: &FsPath,
    agent_home: &FsPath,
    agent_config: &FsPath,
    profile: &ProviderProfile,
    session_key: &str,
    cli_session_id: &str,
    session_marker: &FsPath,
    prompt: &str,
    sink: Option<&PromptStreamSink>,
) -> Result<CliStreamState, String> {
    let mut command = tokio::process::Command::new(binary);
    command
        .env("SUPERCHARGE_HOME", agent_home)
        .arg("--model")
        .arg(&profile.id)
        .arg("--output-format")
        .arg("streaming-json")
        .arg("--single")
        .arg(prompt);
    if session_marker.exists() {
        command.arg("--resume").arg(cli_session_id);
    } else {
        command.arg("--session-id").arg(cli_session_id);
    }
    if let Some(rules) = turn_rules_for(session_key) {
        command.arg("--rules").arg(rules);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| redact_secrets(&error.to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "agent stdout pipe missing".to_owned())?;
    let stderr = child.stderr.take();
    let stream = read_cli_stream(stdout, sink).await?;
    let status = child
        .wait()
        .await
        .map_err(|error| redact_secrets(&error.to_string()))?;
    if !status.success() {
        let mut stderr_text = String::new();
        if let Some(mut pipe) = stderr {
            let _ = tokio::io::AsyncReadExt::read_to_string(&mut pipe, &mut stderr_text).await;
        }
        let stderr = redact_secrets(&stderr_text);
        return Err(if stderr.trim().is_empty() {
            format!("{} exited with status {}", profile.id, status)
        } else {
            format!(
                "{} exited with status {}: {}",
                profile.id,
                status,
                stderr.trim().chars().take(1_000).collect::<String>()
            )
        });
    }
    if let Some(parent) = session_marker.parent() {
        if let Err(error) = std::fs::create_dir_all(parent)
            .and_then(|_| std::fs::write(session_marker, "initialized\n"))
        {
            tracing::warn!(
                session = %session_key,
                path = %session_marker.display(),
                %error,
                "agent response succeeded but its resume marker could not be persisted"
            );
        }
    }
    let _ = agent_config;
    Ok(stream)
}

async fn run_supercharge_prompt_inner(
    session_key: &str,
    prompt: &str,
    sink: Option<PromptStreamSink>,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let metrics = runtime_metrics();
    let started = Instant::now();
    metrics.turns_started.fetch_add(1, Ordering::Relaxed);
    metrics
        .prompt_chars
        .fetch_add(prompt.chars().count() as u64, Ordering::Relaxed);
    let trace_id = uuid::Uuid::new_v4().to_string();
    let span = tracing::info_span!(
        "agent_turn",
        trace_id = %trace_id,
        session = %session_key,
        prompt_chars = prompt.chars().count()
    );
    let result: Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> = async {
        if control_is_emergency_stopped().await {
            return Ok(Some(
                "The daemon is in emergency-stop mode. An operator must call /v1/emergency-resume before new turns can run.".to_owned(),
            ));
        }
        if let Some(reply) = handle_control_text(session_key, prompt).await? {
            return Ok(Some(reply));
        }
        if control_is_stopped(session_key).await {
            return Ok(Some(
                "This session is stopped. Use /start or /new to resume it.".to_owned(),
            ));
        }
        let binary = configured_agent_binary()
            .ok_or_else(|| anyhow::anyhow!("agent binary is not configured"))?;
        let base_config = env::var("SUPERAGENT_PROVIDER_CONFIG_PATH")
            .or_else(|_| env::var("SUPERCHARGE_CONFIG_PATH"))
            .unwrap_or_else(|_| "./runtime/provider-config.toml".to_owned());
        let mut last_error = String::from("all provider attempts failed");
        for profile in provider_profiles_for_turn() {
            let attempt_started = Instant::now();
            let agent_home = agent_runtime_home(&base_config, &profile);
            let agent_config = agent_home.join("config.toml");
            if let Err(error) = write_runtime_config(&agent_config, &profile) {
                last_error = redact_secrets(&error.to_string());
                record_provider_usage(
                    &metrics,
                    &profile.id,
                    prompt.chars().count() as u64,
                    0,
                    attempt_started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    false,
                );
                continue;
            }
            let cli_session_id = agent_session_id(session_key);
            let session_marker = agent_session_marker_path(&agent_home, &cli_session_id);
            let sink_ref = sink.as_ref();
            let attempt = execute_headless_cli_turn(
                FsPath::new(&binary),
                &agent_home,
                &agent_config,
                &profile,
                session_key,
                &cli_session_id,
                &session_marker,
                prompt,
                sink_ref,
            )
            .await;
            let elapsed_ms = attempt_started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            match attempt {
                Ok(stream) => {
                    let reply = stream.live_text.trim().to_owned();
                    record_provider_usage(
                        &metrics,
                        &profile.id,
                        prompt.chars().count() as u64,
                        reply.chars().count() as u64,
                        elapsed_ms,
                        true,
                    );
                    return Ok(if reply.is_empty() { None } else { Some(reply) });
                }
                Err(last_error_msg) => {
                    last_error = last_error_msg;
                    record_provider_usage(
                        &metrics,
                        &profile.id,
                        prompt.chars().count() as u64,
                        0,
                        elapsed_ms,
                        false,
                    );
                }
            }
        }
        Err(last_error.into())
    }
    .instrument(span)
    .await;
    metrics.total_duration_ms.fetch_add(
        started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        Ordering::Relaxed,
    );
    match &result {
        Ok(reply) => {
            metrics.turns_succeeded.fetch_add(1, Ordering::Relaxed);
            if let Some(reply) = reply {
                metrics
                    .reply_chars
                    .fetch_add(reply.chars().count() as u64, Ordering::Relaxed);
            }
        }
        Err(_) => {
            metrics.turns_failed.fetch_add(1, Ordering::Relaxed);
        }
    }
    result
}

async fn run_supercharge_prompt(
    session_key: &str,
    prompt: &str,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    run_supercharge_prompt_inner(session_key, prompt, None).await
}

async fn send_chunked(
    client: &TelegramClient,
    chat_id: i64,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let chars: Vec<char> = text.chars().collect();
    for chunk in chars.chunks(3900) {
        client
            .send_message(chat_id, &chunk.iter().collect::<String>())
            .await?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct WhatsAppVerifyQuery {
    #[serde(rename = "hub.mode")]
    mode: Option<String>,
    #[serde(rename = "hub.verify_token")]
    verify_token: Option<String>,
    #[serde(rename = "hub.challenge")]
    challenge: Option<String>,
}

async fn generic_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let Some((client, config)) = state.webhook.clone() else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "generic webhook is disabled".to_owned(),
            }),
        ));
    };
    client
        .verify_secret(
            &config,
            headers
                .get("x-superagent-webhook-secret")
                .and_then(|value| value.to_str().ok()),
        )
        .map_err(|error| {
            (
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: error.to_string(),
                }),
            )
        })?;
    if let Some(event) = client.parse_event(&config, &body).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: error.to_string(),
            }),
        )
    })? {
        eprintln!(
            "generic webhook event: source={} session={}",
            event.source,
            event.session_key()
        );
        if configured_agent_binary().is_some() {
            let session = event.session_key();
            let prompt = event.text.clone();
            tokio::spawn(async move {
                if let Err(error) = run_supercharge_prompt(&session, &prompt).await {
                    eprintln!("generic webhook agent turn failed: {error}");
                }
            });
        }
    }
    Ok(Json(json!({"ok": true})))
}

async fn telegram_webhook(
    headers: HeaderMap,
    Json(update): Json<TelegramUpdate>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let client = TelegramClient::new(TelegramConfig::from_env().map_err(|error| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: error.to_string(),
            }),
        )
    })?);
    let Some(expected_secret) = client.webhook_secret() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "TELEGRAM_WEBHOOK_SECRET must be configured for webhook mode".to_owned(),
            }),
        ));
    };
    if headers
        .get("X-Telegram-Bot-Api-Secret-Token")
        .and_then(|value| value.to_str().ok())
        != Some(expected_secret)
    {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "invalid Telegram webhook secret".to_owned(),
            }),
        ));
    }
    let Some(message) = client.normalize_update(update) else {
        return Ok(StatusCode::NO_CONTENT);
    };
    if configured_agent_binary().is_some()
        && let Some(reply) = run_supercharge_turn(&message).await.map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: error.to_string(),
                }),
            )
        })?
        && !reply.is_empty()
    {
        let prepared = prepare_reply_for_delivery(&reply).await;
        send_chunked(&client, message.chat_id, &prepared)
            .await
            .map_err(|error| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(ErrorResponse {
                        error: error.to_string(),
                    }),
                )
            })?;
    }
    Ok(StatusCode::ACCEPTED)
}

async fn whatsapp_verify(
    State(state): State<Arc<AppState>>,
    Query(query): Query<WhatsAppVerifyQuery>,
) -> Result<String, StatusCode> {
    let Some(client) = &state.whatsapp else {
        return Err(StatusCode::NOT_FOUND);
    };
    client
        .verify_challenge(
            query.mode.as_deref(),
            query.verify_token.as_deref(),
            query.challenge.as_deref(),
        )
        .ok_or(StatusCode::FORBIDDEN)
}

async fn whatsapp_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let Some(client) = state.whatsapp.clone() else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "WhatsApp gateway is disabled".to_owned(),
            }),
        ));
    };
    client
        .verify_signature(
            &body,
            headers
                .get("x-hub-signature-256")
                .and_then(|value| value.to_str().ok()),
        )
        .map_err(|error| {
            (
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: error.to_string(),
                }),
            )
        })?;
    let payload: Value = serde_json::from_slice(&body).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: error.to_string(),
            }),
        )
    })?;
    for message in client.parse_messages(&payload) {
        let client = client.clone();
        tokio::spawn(async move {
            if let Ok(Some(reply)) =
                run_supercharge_prompt(&message.session_key(), &message.text).await
            {
                let _ = client.send_text(&message.user_id, &reply).await;
            }
        });
    }
    Ok(Json(json!({"ok": true})))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<Option<()>>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

async fn search_memory(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MemoryQuery>,
) -> Json<Vec<MemoryEntry>> {
    let store = state.memory_store.lock().await;
    Json(store.search(query.session_key.as_deref(), &query.q, query.limit))
}

async fn remember_memory(
    State(state): State<Arc<AppState>>,
    Json(request): Json<MemoryRequest>,
) -> Result<Json<MemoryEntry>, (StatusCode, Json<ErrorResponse>)> {
    let mut store = state.memory_store.lock().await;
    let entry = store
        .remember_with_metadata(
            request.session_key,
            request.content,
            request.tags,
            request.kind,
            request.importance,
        )
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: error.to_string(),
                }),
            )
        })?;
    Ok(Json(entry))
}

async fn delete_memory(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let mut store = state.memory_store.lock().await;
    let removed = store.delete(&id).map_err(internal_error)?;
    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "memory entry not found".to_owned(),
            }),
        ))
    }
}

async fn export_memory(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let store = state.memory_store.lock().await;
    Ok(Json(
        json!({ "entries": store.export().map_err(internal_error)? }),
    ))
}

async fn restore_memory(
    State(state): State<Arc<AppState>>,
    Json(request): Json<MemoryRestoreRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    if request.entries.len() > 100_000 {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(ErrorResponse {
                error: "memory restore exceeds the 100000-entry limit".to_owned(),
            }),
        ));
    }
    let mut store = state.memory_store.lock().await;
    let inserted = store
        .restore(request.entries, request.replace)
        .map_err(internal_error)?;
    let (entries, sessions) = store.stats();
    Ok(Json(json!({
        "inserted": inserted,
        "entries": entries,
        "sessions": sessions,
    })))
}

async fn compact_memory(
    State(state): State<Arc<AppState>>,
    Json(request): Json<MemoryCompactRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let mut store = state.memory_store.lock().await;
    let removed = store.compact(request.max_entries).map_err(internal_error)?;
    let (entries, sessions) = store.stats();
    Ok(Json(json!({
        "removed": removed,
        "entries": entries,
        "sessions": sessions,
    })))
}

async fn memory_stats(State(state): State<Arc<AppState>>) -> Json<Value> {
    let store = state.memory_store.lock().await;
    let (entries, sessions) = store.stats();
    Json(json!({ "entries": entries, "sessions": sessions }))
}

async fn list_skills() -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let root = env::var("SUPERAGENT_SKILLS_ROOT").unwrap_or_else(|_| "./skills".to_owned());
    let registry = SkillRegistry::new(root);
    let skills = registry.discover().map_err(internal_error)?;
    Ok(Json(json!({ "skills": skills })))
}

async fn get_skill(
    Path(name): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let root = env::var("SUPERAGENT_SKILLS_ROOT").unwrap_or_else(|_| "./skills".to_owned());
    let registry = SkillRegistry::new(root);
    let skill = registry.load(&name).map_err(|error| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: error.to_string(),
            }),
        )
    })?;
    Ok(Json(serde_json::to_value(skill).map_err(internal_error)?))
}

async fn run_skill(
    Path(name): Path<String>,
    Json(request): Json<SkillRunRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    if request.session_key.trim().is_empty() || request.prompt.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "session_key and prompt are required".to_owned(),
            }),
        ));
    }
    let root = env::var("SUPERAGENT_SKILLS_ROOT").unwrap_or_else(|_| "./skills".to_owned());
    let skill = SkillRegistry::new(root).load(&name).map_err(|error| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: error.to_string(),
            }),
        )
    })?;
    let composed = format!(
        "You are executing the validated skill `{}`.\n\nSkill instructions:\n{}\n\nUser request:\n{}",
        skill.descriptor.name, skill.instructions, request.prompt
    );
    if composed.len() > 768 * 1024 {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(ErrorResponse {
                error: "skill instructions and prompt exceed the 768 KiB limit".to_owned(),
            }),
        ));
    }
    let reply = run_supercharge_prompt(&request.session_key, &composed)
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: error.to_string(),
                }),
            )
        })?;
    Ok(Json(json!({
        "skill": skill.descriptor.name,
        "sha256": skill.sha256,
        "reply": reply,
    })))
}

async fn list_mcp_servers() -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let servers = parse_mcp_servers(env::var("SUPERAGENT_MCP_SERVERS").ok().as_deref())
        .map_err(internal_error)?;
    Ok(Json(json!({ "servers": servers })))
}

async fn list_mcp_tools(
    Query(query): Query<McpToolsQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let servers = parse_mcp_servers(env::var("SUPERAGENT_MCP_SERVERS").ok().as_deref())
        .map_err(internal_error)?;
    let selected = if let Some(name) = query.name {
        servers.get(&name).cloned().into_iter().collect::<Vec<_>>()
    } else {
        servers.values().cloned().collect::<Vec<_>>()
    };
    let mut output = serde_json::Map::new();
    for server in selected {
        if !server.enabled {
            continue;
        }
        let tools = match server.transport.as_str() {
            "stdio" => discover_stdio_tools(&server).await,
            _ => discover_http_tools(&server).await,
        }
        .map_err(internal_error)?;
        output.insert(
            server.name,
            serde_json::to_value(tools).map_err(internal_error)?,
        );
    }
    Ok(Json(Value::Object(output)))
}

async fn call_mcp_tool(
    Json(request): Json<McpToolCallRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let servers = parse_mcp_servers(env::var("SUPERAGENT_MCP_SERVERS").ok().as_deref())
        .map_err(internal_error)?;
    let server = servers.get(&request.server).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "MCP server is not configured".to_owned(),
            }),
        )
    })?;
    let result = match server.transport.as_str() {
        "stdio" => {
            invoke_stdio_tool(
                server,
                &request.tool,
                request.arguments,
                request.timeout_seconds,
            )
            .await
        }
        _ => {
            invoke_http_tool(
                server,
                &request.tool,
                request.arguments,
                request.timeout_seconds,
            )
            .await
        }
    }
    .map_err(internal_error)?;
    Ok(Json(result))
}

async fn run_subagent(
    Json(request): Json<SubagentRequest>,
) -> Result<
    Json<supercharge_superagent::subagents::SubagentResult>,
    (StatusCode, Json<ErrorResponse>),
> {
    let coordinator = global_subagents().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "subagent bridge is not configured".to_owned(),
            }),
        )
    })?;
    let result = coordinator.run(request).await.map_err(subagent_error)?;
    Ok(Json(result))
}

fn subagent_error(error: SubagentError) -> (StatusCode, Json<ErrorResponse>) {
    let status = match error {
        SubagentError::InvalidPrompt | SubagentError::DepthExceeded { .. } => {
            StatusCode::BAD_REQUEST
        }
        SubagentError::Busy => StatusCode::TOO_MANY_REQUESTS,
        SubagentError::NotConfigured => StatusCode::SERVICE_UNAVAILABLE,
        SubagentError::Process(_) | SubagentError::Timeout(_) => StatusCode::BAD_GATEWAY,
    };
    (
        status,
        Json(ErrorResponse {
            error: error.to_string(),
        }),
    )
}

#[cfg(feature = "e2b")]
async fn list_e2b_sessions(State(state): State<Arc<AppState>>) -> Json<Vec<E2bSessionRecord>> {
    let store = state.e2b_sessions.lock().await;
    Json(store.sessions.values().cloned().collect())
}

#[cfg(feature = "e2b")]
async fn get_e2b_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<E2bSessionRecord>, (StatusCode, Json<ErrorResponse>)> {
    let store = state.e2b_sessions.lock().await;
    let Some(session) = store.sessions.get(&id).cloned() else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "E2B session not found".to_owned(),
            }),
        ));
    };
    Ok(Json(session))
}

#[cfg(feature = "e2b")]
async fn create_e2b_session(
    State(state): State<Arc<AppState>>,
    Json(request): Json<E2bSessionRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let session_id = request.session_id.unwrap_or_else(|| {
        format!(
            "e2b-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default()
        )
    });
    let strategy = if request.strategy.as_deref() == Some("sandbox_full") {
        "sandbox_full".to_owned()
    } else {
        "host_only".to_owned()
    };
    let executor = E2bExecutor::new(state.sandbox_plan.clone(), env::var("E2B_API_KEY").ok());
    let status = executor
        .ensure_session(
            request.sandbox_id.as_deref(),
            request.workspace_root.clone(),
        )
        .await
        .map_err(e2b_error)?;
    let now = unix_seconds();
    let record = E2bSessionRecord {
        session_id: session_id.clone(),
        sandbox_id: status.sandbox_id.clone(),
        strategy,
        cwd: request.cwd,
        workspace_root: request.workspace_root,
        created_at: now,
        updated_at: now,
    };
    let mut store = state.e2b_sessions.lock().await;
    store.sessions.insert(session_id, record.clone());
    store.save().map_err(internal_error)?;
    Ok(Json(
        json!({"success": true, "session": record, "status": status}),
    ))
}

#[cfg(feature = "e2b")]
async fn execute_e2b_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<E2bExecRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    if request.command.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "command is required".to_owned(),
            }),
        ));
    }
    let record = {
        let store = state.e2b_sessions.lock().await;
        store.sessions.get(&id).cloned()
    }
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "E2B session not found".to_owned(),
            }),
        )
    })?;
    let cwd = request.cwd.or_else(|| record.cwd.clone());
    let executor = E2bExecutor::new(state.sandbox_plan.clone(), env::var("E2B_API_KEY").ok());
    let result = executor
        .execute_persistent(PersistentExecutionRequest {
            sandbox_id: Some(record.sandbox_id.clone()),
            command: request.command,
            cwd,
            workspace_root: record.workspace_root.clone(),
            timeout_seconds: request.timeout_seconds,
        })
        .await
        .map_err(e2b_error)?;
    let mut store = state.e2b_sessions.lock().await;
    if let Some(current) = store.sessions.get_mut(&id) {
        current.sandbox_id = result
            .sandbox_id
            .clone()
            .unwrap_or_else(|| current.sandbox_id.clone());
        current.updated_at = unix_seconds();
    }
    store.save().map_err(internal_error)?;
    Ok(Json(
        json!({"success": result.exit_code == 0, "result": result, "session": store.sessions.get(&id)}),
    ))
}

#[cfg(feature = "e2b")]
async fn wake_e2b_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let record = {
        let store = state.e2b_sessions.lock().await;
        store.sessions.get(&id).cloned()
    }
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "E2B session not found".to_owned(),
            }),
        )
    })?;
    let executor = E2bExecutor::new(state.sandbox_plan.clone(), env::var("E2B_API_KEY").ok());
    let status = executor
        .ensure_session(Some(&record.sandbox_id), record.workspace_root.clone())
        .await
        .map_err(e2b_error)?;
    let mut store = state.e2b_sessions.lock().await;
    if let Some(current) = store.sessions.get_mut(&id) {
        current.sandbox_id = status.sandbox_id.clone();
        current.updated_at = unix_seconds();
    }
    store.save().map_err(internal_error)?;
    Ok(Json(
        json!({"success": true, "session": store.sessions.get(&id), "status": status}),
    ))
}

#[cfg(feature = "e2b")]
async fn pause_e2b_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let record = {
        let store = state.e2b_sessions.lock().await;
        store.sessions.get(&id).cloned()
    }
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "E2B session not found".to_owned(),
            }),
        )
    })?;
    let executor = E2bExecutor::new(state.sandbox_plan.clone(), env::var("E2B_API_KEY").ok());
    let paused = executor
        .pause(&record.sandbox_id)
        .await
        .map_err(e2b_error)?;
    Ok(Json(
        json!({"success": true, "paused": paused, "session": record}),
    ))
}

#[cfg(feature = "e2b")]
async fn delete_e2b_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let record = {
        let mut store = state.e2b_sessions.lock().await;
        let record = store.sessions.remove(&id);
        store.save().map_err(internal_error)?;
        record
    };
    let Some(record) = record else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "E2B session not found".to_owned(),
            }),
        ));
    };
    let executor = E2bExecutor::new(state.sandbox_plan.clone(), env::var("E2B_API_KEY").ok());
    executor.kill(&record.sandbox_id).await.map_err(e2b_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "e2b")]
fn e2b_error(error: supercharge_superagent::E2bError) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_GATEWAY,
        Json(ErrorResponse {
            error: error.to_string(),
        }),
    )
}

fn configured_agent_binary() -> Option<std::ffi::OsString> {
    env::var_os("SUPERAGENT_AGENT_BINARY").or_else(|| env::var_os("SUPERCHARGE_AGENT_BINARY"))
}

fn agent_runtime_home(base_config: &str, profile: &ProviderProfile) -> PathBuf {
    let profile_id = profile
        .id
        .chars()
        .map(|character| if character.is_ascii_alphanumeric() { character } else { '_' })
        .collect::<String>();
    PathBuf::from(base_config)
        .parent()
        .unwrap_or_else(|| FsPath::new("."))
        .join("agent-runtime")
        .join(profile_id)
}

fn agent_session_id(session_key: &str) -> String {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!("supercharge-superagent-session:{session_key}").as_bytes(),
    )
    .to_string()
}

fn agent_session_marker_path(agent_home: &FsPath, session_id: &str) -> PathBuf {
    agent_home.join("daemon-sessions").join(session_id)
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

fn internal_error(error: impl std::fmt::Display) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: error.to_string(),
        }),
    )
}

async fn provider_health() -> Json<Value> {
    let profile = configured_profile();
    Json(serde_json::to_value(health_check(&profile).await).unwrap_or_else(|_| json!({"ok":false})))
}

async fn list_providers() -> Json<Value> {
    Json(json!({
        "active": configured_profile(),
        "providers": builtin_profiles(),
    }))
}

async fn configure_provider(
    Json(request): Json<ProviderConfigRequest>,
) -> Result<Json<ProviderProfile>, (StatusCode, Json<ErrorResponse>)> {
    let profile = validate_request(request).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: error.to_string(),
            }),
        )
    })?;
    let path = env::var("SUPERAGENT_PROVIDER_CONFIG_PATH")
        .or_else(|_| env::var("SUPERCHARGE_CONFIG_PATH"))
        .unwrap_or_else(|_| "./runtime/provider-config.toml".to_owned());
    write_runtime_config(path, &profile).map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: error.to_string(),
            }),
        )
    })?;
    Ok(Json(profile))
}

async fn connection_status() -> Json<Vec<ConnectionReadiness>> {
    Json(vec![
        connection_readiness("http-auth", "protected-api", &["SUPERAGENT_API_TOKEN"]),
        connection_readiness(
            "provider",
            "agent-runtime",
            &["POLLINATIONS_API_KEY", "SUPERAGENT_AGENT_BINARY"],
        ),
        connection_readiness("e2b", "sandbox-api", &["E2B_API_KEY"]),
        connection_readiness("telegram", "long-polling", &["TELEGRAM_BOT_TOKEN"]),
        connection_readiness("discord", "gateway-websocket", &["DISCORD_BOT_TOKEN"]),
        connection_readiness(
            "slack",
            "socket-mode",
            &["SLACK_APP_TOKEN", "SLACK_BOT_TOKEN"],
        ),
        connection_readiness(
            "whatsapp",
            "cloud-webhook",
            &[
                "WHATSAPP_ACCESS_TOKEN",
                "WHATSAPP_PHONE_NUMBER_ID",
                "WHATSAPP_VERIFY_TOKEN",
            ],
        ),
        connection_readiness(
            "signal",
            "signal-cli-rest",
            &["SIGNAL_ACCOUNT", "SIGNAL_CLI_BASE_URL"],
        ),
        connection_readiness(
            "matrix",
            "client-server-sync",
            &["MATRIX_HOMESERVER", "MATRIX_ACCESS_TOKEN", "MATRIX_USER_ID"],
        ),
        connection_readiness(
            "generic-webhook",
            "authenticated-webhook",
            &["GENERIC_WEBHOOK_SECRET"],
        ),
        connection_readiness(
            "email",
            "imap-smtp",
            &["EMAIL_USERNAME", "EMAIL_PASSWORD", "EMAIL_ALLOWED_SENDERS"],
        ),
    ])
}

async fn emergency_stop() -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    set_emergency_stopped(true).await.map_err(internal_error)?;
    Ok(Json(json!({"emergency_stopped": true})))
}

async fn emergency_resume() -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    set_emergency_stopped(false).await.map_err(internal_error)?;
    Ok(Json(json!({"emergency_stopped": false})))
}

async fn observability(State(state): State<Arc<AppState>>) -> Json<ObservabilityResponse> {
    let store = state.job_store.lock().await;
    let mut queued_jobs = 0;
    let mut running_jobs = 0;
    let mut succeeded_jobs = 0;
    let mut failed_jobs = 0;
    for job in store.all() {
        match job.status {
            JobStatus::Queued => queued_jobs += 1,
            JobStatus::Running => running_jobs += 1,
            JobStatus::Succeeded => succeeded_jobs += 1,
            JobStatus::Failed | JobStatus::Cancelled => failed_jobs += 1,
        }
    }
    drop(store);
    let (memory_entries, memory_sessions) = state.memory_store.lock().await.stats();
    let schedules = state.schedule_store.lock().await;
    let schedule_list: Vec<_> = schedules.list().collect();
    let enabled_schedules = schedule_list.iter().filter(|job| job.enabled).count();
    Json(ObservabilityResponse {
        uptime_seconds: unix_seconds().saturating_sub(state.started_at),
        active_gateways: configured_gateways(),
        queued_jobs,
        running_jobs,
        succeeded_jobs,
        failed_jobs,
        memory_entries,
        memory_sessions,
        schedules: schedule_list.len(),
        enabled_schedules,
        turns_started: state.metrics.turns_started.load(Ordering::Relaxed),
        turns_succeeded: state.metrics.turns_succeeded.load(Ordering::Relaxed),
        turns_failed: state.metrics.turns_failed.load(Ordering::Relaxed),
        prompt_chars: state.metrics.prompt_chars.load(Ordering::Relaxed),
        reply_chars: state.metrics.reply_chars.load(Ordering::Relaxed),
        total_duration_ms: state.metrics.total_duration_ms.load(Ordering::Relaxed),
        provider_usage: state
            .metrics
            .provider_usage
            .lock()
            .map(|usage| usage.clone())
            .unwrap_or_default(),
    })
}

fn configured_gateways() -> Vec<String> {
    [
        (env::var("TELEGRAM_BOT_TOKEN").is_ok(), "telegram"),
        (env::var("DISCORD_BOT_TOKEN").is_ok(), "discord"),
        (env::var("SLACK_APP_TOKEN").is_ok(), "slack"),
        (env::var("WHATSAPP_ACCESS_TOKEN").is_ok(), "whatsapp"),
        (env::var("SIGNAL_ACCOUNT").is_ok(), "signal"),
        (env::var("MATRIX_ACCESS_TOKEN").is_ok(), "matrix"),
        (
            env::var("EMAIL_USERNAME").is_ok() && env::var("EMAIL_PASSWORD").is_ok(),
            "email",
        ),
        (env::var("GENERIC_WEBHOOK_SECRET").is_ok(), "webhook"),
    ]
    .into_iter()
    .filter_map(|(enabled, name)| enabled.then_some(name.to_owned()))
    .collect()
}

async fn daemon_status(State(state): State<Arc<AppState>>) -> Json<DaemonStatus> {
    let store = state.job_store.lock().await;
    let mut status = DaemonStatus {
        daemon: "running",
        gateways: Vec::new(),
        queued_jobs: 0,
        running_jobs: 0,
        succeeded_jobs: 0,
        failed_jobs: 0,
    };
    status.gateways = configured_gateways();
    for job in store.all() {
        match job.status {
            JobStatus::Queued => status.queued_jobs += 1,
            JobStatus::Running => status.running_jobs += 1,
            JobStatus::Succeeded => status.succeeded_jobs += 1,
            JobStatus::Failed | JobStatus::Cancelled => status.failed_jobs += 1,
        }
    }
    Json(status)
}

async fn parse_media(
    Json(request): Json<MediaParseRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let root = env::var("SUPERAGENT_MEDIA_ROOT").unwrap_or_else(|_| "./runtime/media".to_owned());
    let max_bytes = env::var("SUPERAGENT_MAX_MEDIA_BYTES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(25 * 1024 * 1024);
    let (text, media) =
        extract_media(request.response.as_str(), root, max_bytes).map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: error.to_string(),
                }),
            )
        })?;
    Ok(Json(json!({ "text": text, "media": media })))
}

async fn list_schedules(State(state): State<Arc<AppState>>) -> Json<Vec<ScheduledJob>> {
    let store = state.schedule_store.lock().await;
    Json(store.list().cloned().collect())
}

async fn create_schedule(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ScheduleRequest>,
) -> Result<Json<ScheduledJob>, (StatusCode, Json<ErrorResponse>)> {
    if request.name.trim().is_empty() || request.prompt.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "schedule name and prompt are required".to_owned(),
            }),
        ));
    }
    let mut store = state.schedule_store.lock().await;
    let job = store
        .create(
            request.name,
            request.prompt,
            request.interval_seconds,
            request.cron,
            request.home_channel,
        )
        .map_err(internal_error)?;
    Ok(Json(job))
}

async fn delete_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let id = uuid::Uuid::parse_str(&id).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: error.to_string(),
            }),
        )
    })?;
    let mut store = state.schedule_store.lock().await;
    if store.delete(&id).map_err(internal_error)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "schedule not found".to_owned(),
            }),
        ))
    }
}

async fn toggle_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<ScheduleToggleRequest>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let id = uuid::Uuid::parse_str(&id).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: error.to_string(),
            }),
        )
    })?;
    let mut store = state.schedule_store.lock().await;
    if store
        .set_enabled(&id, request.enabled)
        .map_err(internal_error)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "schedule not found".to_owned(),
            }),
        ))
    }
}

async fn list_jobs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<AgentJob>>, (StatusCode, Json<ErrorResponse>)> {
    let store = state.job_store.lock().await;
    Ok(Json(store.all().cloned().collect()))
}

async fn get_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<AgentJob>, (StatusCode, Json<ErrorResponse>)> {
    let store = state.job_store.lock().await;
    let Some(job) = store.get(&id).cloned() else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "job not found".to_owned(),
            }),
        ));
    };
    Ok(Json(job))
}

async fn enqueue_job(
    State(state): State<Arc<AppState>>,
    Json(request): Json<EnqueueRequest>,
) -> Result<Json<AgentJob>, (StatusCode, Json<ErrorResponse>)> {
    let mut store = state.job_store.lock().await;
    let job = store
        .enqueue(
            request.session_key,
            request.platform,
            request.chat_id,
            request.prompt,
            request.max_attempts,
        )
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: error.to_string(),
                }),
            )
        })?;
    Ok(Json(job))
}

async fn browser_status() -> Result<Json<StreamStatus>, StatusCode> {
    if !browser::is_e2b_runtime() {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(browser::read_stream_status(browser::stream_port()).await))
}

async fn browser_stream() -> Result<Response, StatusCode> {
    if !browser::is_e2b_runtime() {
        return Err(StatusCode::NOT_FOUND);
    }
    let port = browser::stream_port();
    let status = browser::ensure_stream(port).await;
    if !status.connected {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let mut response = Response::new(Body::from_stream(browser::sse_stream(status)));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    response
        .headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    Ok(response)
}

#[derive(Debug, Deserialize)]
struct BrowserControlRequest {
    #[serde(default)]
    approved: bool,
    #[serde(flatten)]
    action: browser::BrowserAction,
}

async fn browser_control(
    Json(request): Json<BrowserControlRequest>,
) -> Result<Json<browser::BrowserActionResult>, (StatusCode, Json<ErrorResponse>)> {
    if !browser::is_e2b_runtime() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse { error: "browser control is available only in E2B mode".to_owned() }),
        ));
    }
    let enabled = env::var("SUPERAGENT_BROWSER_CONTROL_ENABLED")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if !enabled {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "browser control is disabled by SUPERAGENT_BROWSER_CONTROL_ENABLED"
                    .to_owned(),
            }),
        ));
    }
    if !request.approved {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "browser actions require explicit approval".to_owned(),
            }),
        ));
    }
    let result = browser::execute_action(request.action)
        .await
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error })))?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct BrowserHandoffRequest {
    action: String,
    #[serde(default)]
    minutes: Option<u8>,
}

async fn browser_handoff(
    Json(request): Json<BrowserHandoffRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    if !browser::is_e2b_runtime() {
        return Err((StatusCode::NOT_FOUND, Json(ErrorResponse { error: "browser handoff is available only in E2B mode".to_owned() })));
    }
    let start = match request.action.as_str() {
        "start" => true,
        "stop" => false,
        _ => return Err((StatusCode::BAD_REQUEST, Json(ErrorResponse { error: "action must be start or stop".to_owned() }))),
    };
    let value = browser::handoff(start, request.minutes)
        .await
        .map_err(|error| (StatusCode::SERVICE_UNAVAILABLE, Json(ErrorResponse { error })))?;
    Ok(Json(value))
}

async fn browser_vnc(ws: WebSocketUpgrade) -> Result<Response, StatusCode> {
    if !browser::is_e2b_runtime() {
        return Err(StatusCode::NOT_FOUND);
    }
    if !browser::handoff_active().await.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)? {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(ws.on_upgrade(browser_vnc_relay).into_response())
}

async fn browser_vnc_relay(socket: axum::extract::ws::WebSocket) {
    let Ok((upstream, _)) = connect_async("ws://127.0.0.1:6080/websockify").await else {
        return;
    };
    let (mut client_send, mut client_recv) = socket.split();
    let (mut vnc_send, mut vnc_recv) = upstream.split();
    let client_to_vnc = async {
        while let Some(message) = client_recv.next().await {
            let Ok(message) = message else { break };
            let converted = match message {
                Message::Text(value) => TungsteniteMessage::Text(value.to_string().into()),
                Message::Binary(value) => TungsteniteMessage::Binary(value),
                Message::Ping(value) => TungsteniteMessage::Ping(value),
                Message::Pong(value) => TungsteniteMessage::Pong(value),
                Message::Close(_) => TungsteniteMessage::Close(None),
            };
            if vnc_send.send(converted).await.is_err() { break; }
        }
    };
    let vnc_to_client = async {
        while let Some(message) = vnc_recv.next().await {
            let Ok(message) = message else { break };
            let converted = match message {
                TungsteniteMessage::Text(value) => Message::Text(value.to_string().into()),
                TungsteniteMessage::Binary(value) => Message::Binary(value),
                TungsteniteMessage::Ping(value) => Message::Ping(value),
                TungsteniteMessage::Pong(value) => Message::Pong(value),
                TungsteniteMessage::Close(_) => Message::Close(None),
                TungsteniteMessage::Frame(_) => continue,
            };
            if client_send.send(converted).await.is_err() { break; }
        }
    };
    tokio::select! {
        _ = client_to_vnc => {},
        _ = vnc_to_client => {},
    }
}

#[derive(Debug, Deserialize)]
struct AppChatQuery {
    #[serde(alias = "chatId")]
    chat_id: Option<String>,
    since: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct AppChatFile {
    name: Option<String>,
    content_base64: Option<String>,
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AppChatRequest {
    text: Option<String>,
    #[serde(alias = "chatId")]
    chat_id: Option<String>,
    files: Option<Vec<AppChatFile>>,
}

#[derive(Debug, Deserialize)]
struct SwitchConversationRequest {
    #[serde(rename = "sessionId")]
    session_id: String,
}

/// The cloud-infra mobile app listens on the owner conversation unless it
/// explicitly supplies a chat id. Keep daemon defaults aligned with that
/// contract so a submitted turn and its event stream use the same channel.
fn app_chat_id(chat_id: Option<String>) -> String {
    chat_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "owner".to_owned())
}

const APP_EVENT_LOG_LIMIT: usize = 512;

fn app_event_id(event: &Value) -> u64 {
    event.get("id").and_then(Value::as_u64).unwrap_or(0)
}

fn app_events_for_replay<'a>(
    events: impl IntoIterator<Item = &'a Value>,
    chat_id: &str,
    since: u64,
) -> Vec<Value> {
    events
        .into_iter()
        .filter(|event| {
            event.get("chatId").and_then(Value::as_str) == Some(chat_id)
                && app_event_id(event) > since
        })
        .cloned()
        .collect()
}

fn retain_app_event(log: &mut VecDeque<Value>, event: Value) {
    if log.len() >= APP_EVENT_LOG_LIMIT {
        log.pop_front();
    }
    log.push_back(event);
}

fn app_session_title(prompt: &str) -> String {
    let compact = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title = compact.chars().take(60).collect::<String>();
    if compact.chars().count() > 60 {
        title.push('…');
    }
    if title.is_empty() {
        "Conversation".to_owned()
    } else {
        title
    }
}

async fn app_active_session(state: &AppState) -> String {
    state.active_app_session.lock().await.clone()
}

async fn app_record_message(state: &AppState, session_id: &str, role: &str, content: &str) {
    let now = unix_seconds();
    let mut sessions = state.sessions.lock().await;
    let session = sessions
        .entry(session_id.to_owned())
        .or_insert_with(|| DaemonSession {
            id: session_id.to_owned(),
            created_at: now,
            updated_at: now,
            title: app_session_title(content),
            status: default_session_status(),
            messages: Vec::new(),
        });
    session.updated_at = now;
    if session.title.is_empty() || session.title == "New conversation" {
        session.title = app_session_title(content);
    }
    session.messages.push(ConversationMessage {
        role: role.to_owned(),
        content: content.to_owned(),
    });
    let _ = persist_daemon_sessions(&state.session_state_path, &sessions);
}

fn app_conversation_summary(session: &DaemonSession) -> Value {
    json!({
        "sessionId": session.id,
        "title": if session.title.is_empty() { "Conversation" } else { &session.title },
        "createdAt": session.created_at.saturating_mul(1000),
        "updatedAt": session.updated_at.saturating_mul(1000),
        "status": if session.status.is_empty() { "active" } else { &session.status },
    })
}

fn app_turn_completion_events() -> [Value; 2] {
    [
        json!({"type":"status", "busy":false}),
        json!({"type":"done"}),
    ]
}

fn app_reply_completion_events(text: &str) -> [Value; 3] {
    [
        json!({"type":"text", "text":text, "final":true}),
        json!({"type":"status", "busy":false}),
        json!({"type":"done"}),
    ]
}

fn app_emit(state: &AppState, chat_id: &str, mut payload: Value) {
    let Some(object) = payload.as_object_mut() else {
        return;
    };
    let id = state.app_event_seq.fetch_add(1, Ordering::Relaxed) + 1;
    object.insert("id".to_owned(), json!(id));
    object.insert("ts".to_owned(), json!(unix_seconds()));
    object.insert("chatId".to_owned(), json!(chat_id));
    if let Ok(mut log) = state.app_event_log.lock() {
        retain_app_event(&mut log, payload.clone());
    }
    let _ = state.app_events.send(payload);
}

fn app_workspace_root() -> std::path::PathBuf {
    env::var("SUPERAGENT_SHARE_WORKSPACE_ROOT")
        .or_else(|_| env::var("SUPERAGENT_DAEMON_WORKDIR"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/home/user/workspace"))
}

fn save_app_uploads(files: &[AppChatFile]) -> Result<Vec<String>, String> {
    let root = app_workspace_root();
    let inbox = root.join(".superagent-inbox");
    std::fs::create_dir_all(&inbox).map_err(|error| error.to_string())?;
    let mut paths = Vec::new();
    for (index, file) in files.iter().take(12).enumerate() {
        let name = file
            .name
            .as_deref()
            .and_then(|value| std::path::Path::new(value).file_name())
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty() && *value != "." && *value != "..")
            .unwrap_or("upload.bin");
        if let Some(url) = file
            .url
            .as_deref()
            .filter(|value| value.starts_with("https://"))
        {
            paths.push(format!("{name} (remote URL: {url})"));
            continue;
        }
        let encoded = file.content_base64.as_deref().unwrap_or_default();
        let raw = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| format!("invalid upload {name}: {error}"))?;
        if raw.len() > 25 * 1024 * 1024 {
            return Err(format!("upload {name} exceeds 25 MiB"));
        }
        let path = inbox.join(format!("{}-{name}", unix_seconds() + index as u64));
        std::fs::write(&path, raw).map_err(|error| error.to_string())?;
        paths.push(path.to_string_lossy().into_owned());
    }
    Ok(paths)
}

fn spawn_app_turn(state: Arc<AppState>, chat_id: String, session_id: String, prompt: String) {
    app_emit(&state, &chat_id, json!({"type":"status", "busy":true}));
    tokio::spawn(async move {
        let session_key = format!("app:{session_id}");
        let result = run_supercharge_prompt_inner(
            &session_key,
            &prompt,
            Some(PromptStreamSink::App {
                state: state.clone(),
                chat_id: chat_id.clone(),
            }),
        )
        .await;
        match result {
            Ok(Some(reply)) if !reply.trim().is_empty() => {
                let prepared = prepare_reply_for_delivery(&reply).await;
                app_record_message(&state, &session_id, "assistant", &prepared).await;
                for event in app_reply_completion_events(&prepared) {
                    app_emit(&state, &chat_id, event);
                }
            }
            Ok(_) => {
                for event in app_reply_completion_events("") {
                    app_emit(&state, &chat_id, event);
                }
            }
            Err(error) => {
                app_emit(
                    &state,
                    &chat_id,
                    json!({"type":"error", "text":error.to_string()}),
                );
                for event in app_turn_completion_events() {
                    app_emit(&state, &chat_id, event);
                }
            }
        }
    });
}

async fn list_sessions(State(state): State<Arc<AppState>>) -> Json<Vec<DaemonSession>> {
    let sessions = state.sessions.lock().await;
    Json(sessions.values().cloned().collect())
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<ErrorResponse>)> {
    let id = request
        .id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("session-{}", uuid::Uuid::new_v4()));
    if id.len() > 128 || id.chars().any(char::is_control) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid session id".to_owned(),
            }),
        ));
    }
    let now = unix_seconds();
    let session = DaemonSession {
        id: id.clone(),
        created_at: now,
        updated_at: now,
        title: "Conversation".to_owned(),
        status: default_session_status(),
        messages: Vec::new(),
    };
    {
        let mut sessions = state.sessions.lock().await;
        sessions.insert(id, session.clone());
        persist_daemon_sessions(&state.session_state_path, &sessions).map_err(internal_error)?;
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({"ok": true, "session": session})),
    ))
}

async fn app_status(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "usage": {},
        "turn": {"busy": false},
        "daemon": "running",
        "uptimeSeconds": unix_seconds().saturating_sub(state.started_at),
    }))
}

async fn app_commands() -> Json<Value> {
    Json(json!({"commands":[
        {"command":"/new", "description":"Start a new conversation"},
        {"command":"/start", "description":"Resume this conversation"},
        {"command":"/stop", "description":"Pause agent turns"},
        {"command":"/status", "description":"Show daemon control state"},
        {"command":"/model", "description":"Inspect or set the requested model"},
        {"command":"/update", "description":"Check for a Supercharge binary update (E2B)"},
        {"command":"/help", "description":"Show available commands"}
    ]}))
}

async fn list_app_conversations(State(state): State<Arc<AppState>>) -> Json<Value> {
    let active = app_active_session(&state).await;
    let sessions = state.sessions.lock().await;
    let mut conversations = sessions
        .values()
        .map(app_conversation_summary)
        .collect::<Vec<_>>();
    conversations.sort_by(|left, right| {
        right
            .get("updatedAt")
            .and_then(Value::as_u64)
            .cmp(&left.get("updatedAt").and_then(Value::as_u64))
    });
    Json(json!({"active":active,"conversations":conversations}))
}

async fn get_app_conversation(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let sessions = state.sessions.lock().await;
    let session = sessions.get(&session_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "conversation not found".to_owned(),
            }),
        )
    })?;
    let messages = session
        .messages
        .iter()
        .map(|message| json!({"role":message.role,"content":message.content}))
        .collect::<Vec<_>>();
    Ok(Json(json!({"sessionId":session.id,"messages":messages})))
}

async fn create_app_conversation(State(state): State<Arc<AppState>>) -> Json<Value> {
    let now = unix_seconds();
    let previous = app_active_session(&state).await;
    let session_id = format!("chat-{}", uuid::Uuid::new_v4());
    let conversations = {
        let mut sessions = state.sessions.lock().await;
        if let Some(current) = sessions.get_mut(&previous) {
            current.status = "closed".to_owned();
            current.updated_at = now;
        }
        sessions.insert(
            session_id.clone(),
            DaemonSession {
                id: session_id.clone(),
                created_at: now,
                updated_at: now,
                title: "New conversation".to_owned(),
                status: default_session_status(),
                messages: Vec::new(),
            },
        );
        let _ = persist_daemon_sessions(&state.session_state_path, &sessions);
        sessions
            .values()
            .map(app_conversation_summary)
            .collect::<Vec<_>>()
    };
    *state.active_app_session.lock().await = session_id.clone();
    app_emit(
        &state,
        "owner",
        json!({"type":"conversation","sessionId":session_id,"previousSessionId":previous,"conversations":conversations}),
    );
    Json(json!({"active":app_active_session(&state).await,"conversations":conversations}))
}

async fn switch_app_conversation(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SwitchConversationRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    if request.session_id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "sessionId is required".to_owned(),
            }),
        ));
    }
    let now = unix_seconds();
    let previous = app_active_session(&state).await;
    let conversations = {
        let mut sessions = state.sessions.lock().await;
        if !sessions.contains_key(&request.session_id) {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "conversation not found".to_owned(),
                }),
            ));
        }
        for (id, session) in sessions.iter_mut() {
            session.status = if id == &request.session_id {
                "active"
            } else {
                "closed"
            }
            .to_owned();
            if id == &request.session_id {
                session.updated_at = now;
            }
        }
        let _ = persist_daemon_sessions(&state.session_state_path, &sessions);
        sessions
            .values()
            .map(app_conversation_summary)
            .collect::<Vec<_>>()
    };
    *state.active_app_session.lock().await = request.session_id.clone();
    app_emit(
        &state,
        "owner",
        json!({"type":"conversation","sessionId":request.session_id,"previousSessionId":previous,"conversations":conversations}),
    );
    Ok(Json(
        json!({"active":app_active_session(&state).await,"conversations":conversations}),
    ))
}

async fn app_chat(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AppChatRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<ErrorResponse>)> {
    let explicit_chat_id = request.chat_id.filter(|value| !value.trim().is_empty());
    let chat_id = app_chat_id(explicit_chat_id.clone());
    let session_id = match explicit_chat_id {
        Some(value) => value,
        None => app_active_session(&state).await,
    };
    if chat_id.len() > 128 || chat_id.chars().any(char::is_control) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid chat id".to_owned(),
            }),
        ));
    }
    let text = request.text.unwrap_or_default().trim().to_owned();
    let files = request.files.unwrap_or_default();
    let paths = save_app_uploads(&files)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error })))?;
    if text.is_empty() && paths.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "text or files required".to_owned(),
            }),
        ));
    }
    let mut prompt = text;
    if !paths.is_empty() {
        if !prompt.is_empty() {
            prompt.push_str("\\n\\n");
        }
        prompt.push_str(
            "User-provided files are available in the workspace. Inspect them when relevant:\\n",
        );
        prompt.push_str(
            &paths
                .iter()
                .map(|path| format!("- {path}"))
                .collect::<Vec<_>>()
                .join("\\n"),
        );
    }
    {
        let now = unix_seconds();
        let mut sessions = state.sessions.lock().await;
        let session = sessions.entry(session_id.clone()).or_insert(DaemonSession {
            id: session_id.clone(),
            created_at: now,
            updated_at: now,
            title: app_session_title(&prompt),
            status: default_session_status(),
            messages: Vec::new(),
        });
        session.updated_at = now;
        persist_daemon_sessions(&state.session_state_path, &sessions).map_err(internal_error)?;
    }
    app_record_message(&state, &session_id, "user", &prompt).await;
    spawn_app_turn(state, chat_id.clone(), session_id.clone(), prompt);
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"ok":true,"chatId":chat_id,"sessionId":session_id})),
    ))
}

async fn app_chat_events(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AppChatQuery>,
) -> Response {
    let chat_id = app_chat_id(query.chat_id);
    let since = query.since.unwrap_or(0);
    // Subscribe before snapshot so live events cannot slip between replay and
    // the broadcast receiver. The Vectra app reconnects after Vercel's ~55s
    // SSE cap; without replay those turns look like “no reply”.
    let mut receiver = state.app_events.subscribe();
    let replay = {
        let log = state
            .app_event_log
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        app_events_for_replay(log.iter(), &chat_id, since)
    };
    let replayed_ids: HashSet<u64> = replay.iter().map(app_event_id).collect();
    let stream = async_stream::stream! {
        yield Ok::<Bytes, Infallible>(app_sse_data_frame(&json!({"type":"ready","id":0,"chatId":chat_id})));
        for event in replay {
            yield Ok::<Bytes, Infallible>(app_sse_data_frame(&event));
        }
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(15), receiver.recv()).await {
                Ok(Ok(event)) => {
                    let same_chat = event.get("chatId").and_then(Value::as_str) == Some(chat_id.as_str());
                    let event_id = app_event_id(&event);
                    let newer = event_id > since;
                    if same_chat && newer && !replayed_ids.contains(&event_id) {
                        yield Ok::<Bytes, Infallible>(app_sse_data_frame(&event));
                    }
                }
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                    yield Ok::<Bytes, Infallible>(Bytes::from(": lagged\n\n"));
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => break,
                Err(_) => yield Ok::<Bytes, Infallible>(Bytes::from(": ping\n\n")),
            }
        }
    };
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    response
        .headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    response
}

fn app_sse_data_frame(event: &Value) -> Bytes {
    Bytes::from(format!("data: {event}\n\n"))
}

async fn api_stream(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(query): Query<AppChatQuery>,
) -> impl IntoResponse {
    let chat_id = app_chat_id(query.chat_id);
    ws.on_upgrade(move |mut socket| async move {
        let mut receiver = state.app_events.subscribe();
        let ready = json!({"type": "ready", "id": 0, "chatId": chat_id});
        if socket.send(Message::Text(ready.to_string().into())).await.is_err() {
            return;
        }
        loop {
            tokio::select! {
                incoming = socket.recv() => {
                    match incoming {
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Ok(_)) => {}
                        Some(Err(_)) => break,
                    }
                }
                event = receiver.recv() => {
                    match event {
                        Ok(event) if event.get("chatId").and_then(Value::as_str) == Some(chat_id.as_str()) => {
                            if socket.send(Message::Text(event.to_string().into())).await.is_err() {
                                break;
                            }
                        }
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    })
}

async fn app_chat_stop(
    State(state): State<Arc<AppState>>,
    Json(request): Json<Value>,
) -> Json<Value> {
    let chat_id = request
        .get("chatId")
        .and_then(Value::as_str)
        .unwrap_or("owner");
    app_emit(
        &state,
        chat_id,
        json!({"type":"status", "busy":false, "stopped":true}),
    );
    Json(json!({"ok":true}))
}

async fn app_chat_approve(Json(_request): Json<Value>) -> Json<Value> {
    Json(json!({"ok":true}))
}

async fn app_chat_answer(Json(_request): Json<Value>) -> Json<Value> {
    Json(json!({"ok":true}))
}

async fn app_chat_pick(Json(_request): Json<Value>) -> Json<Value> {
    Json(json!({"ok":true}))
}

async fn add_security_headers(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

async fn auth(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if let Some(expected) = &state.api_token {
        let supplied = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        if supplied != Some(expected.as_str()) {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    let session = request
        .headers()
        .get("x-superagent-session")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .unwrap_or("api");
    if !state
        .rate_limiter
        .lock()
        .await
        .allow(session, unix_seconds())
    {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    Ok(next.run(request).await)
}

async fn healthz(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        config_fingerprint: state.config.fingerprint(),
    })
}

async fn capabilities(State(state): State<Arc<AppState>>) -> Json<SuperagentConfig> {
    Json(state.config.clone())
}

async fn check_approval(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ApprovalQuery>,
) -> Json<ApprovalResponse> {
    match state.approval.validate(&query.command, query.approved) {
        Ok(()) => Json(ApprovalResponse {
            allowed: true,
            reason: None,
        }),
        Err(error) => Json(ApprovalResponse {
            allowed: false,
            reason: Some(error.to_string()),
        }),
    }
}

async fn execute(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ExecuteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    if let Err(error) = state.approval.validate(&payload.command, payload.approved) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: error.to_string(),
            }),
        ));
    }

    #[cfg(feature = "e2b")]
    {
        let api_key = env::var("E2B_API_KEY").ok();
        let executor =
            supercharge_superagent::E2bExecutor::new(state.sandbox_plan.clone(), api_key);
        let result = executor.execute(&payload.command).await.map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: error.to_string(),
                }),
            )
        })?;
        Ok(Json(
            serde_json::to_value(result).expect("result is serializable"),
        ))
    }

    #[cfg(not(feature = "e2b"))]
    {
        let _ = payload;
        Err((
            StatusCode::NOT_IMPLEMENTED,
            Json(ErrorResponse {
                error: "E2B support is disabled; rebuild with --features e2b".to_owned(),
            }),
        ))
    }
}

#[allow(dead_code)]
fn _infallible(_: Infallible) {}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        DaemonSession, RateLimiter, agent_session_id, app_chat_id, app_events_for_replay,
        app_reply_completion_events, app_sse_data_frame, app_turn_completion_events,
        connection_readiness, default_session_status,
        load_daemon_sessions, persist_daemon_sessions,
    };
    use std::collections::BTreeMap;

    #[test]
    fn rate_limiter_scopes_requests_by_session_and_resets_window() {
        let mut limiter = RateLimiter {
            per_minute: 2,
            buckets: HashMap::new(),
        };
        assert!(limiter.allow("alpha", 100));
        assert!(limiter.allow("alpha", 101));
        assert!(!limiter.allow("alpha", 102));
        assert!(limiter.allow("beta", 102));
        assert!(limiter.allow("alpha", 160));
    }

    #[test]
    fn app_chat_default_matches_the_cloud_infra_owner_conversation() {
        assert_eq!(app_chat_id(None), "owner");
        assert_eq!(app_chat_id(Some("  ".to_owned())), "owner");
        assert_eq!(app_chat_id(Some("project-42".to_owned())), "project-42");
    }

    #[test]
    fn app_turn_completion_matches_mobile_terminal_event_semantics() {
        let events = app_turn_completion_events();
        assert_eq!(events[0]["type"], "status");
        assert_eq!(events[0]["busy"], false);
        assert_eq!(events[1]["type"], "done");
    }

    #[test]
    fn app_reply_event_sequence_sends_final_text_before_completion() {
        let events = app_reply_completion_events("mobile-visible answer");
        assert_eq!(events[0]["type"], "text");
        assert_eq!(events[0]["text"], "mobile-visible answer");
        assert_eq!(events[0]["final"], true);
        assert_eq!(events[1]["type"], "status");
        assert_eq!(events[1]["busy"], false);
        assert_eq!(events[2]["type"], "done");
    }

    #[test]
    fn daemon_session_keys_map_to_stable_cli_compatible_uuids() {
        let owner = agent_session_id("app:owner");
        assert!(uuid::Uuid::try_parse(&owner).is_ok());
        assert_eq!(owner, agent_session_id("app:owner"));
        assert_ne!(owner, agent_session_id("app:another-conversation"));
    }

    #[test]
    fn app_event_replay_returns_later_events_for_the_same_chat() {
        let events = vec![
            serde_json::json!({"id":1,"chatId":"owner","type":"status","busy":true}),
            serde_json::json!({"id":2,"chatId":"other","type":"text","text":"nope"}),
            serde_json::json!({"id":3,"chatId":"owner","type":"text","text":"hello"}),
            serde_json::json!({"id":4,"chatId":"owner","type":"done"}),
        ];
        let replay = app_events_for_replay(&events, "owner", 1);
        assert_eq!(replay.len(), 2);
        assert_eq!(replay[0]["type"], "text");
        assert_eq!(replay[1]["type"], "done");
    }

    #[test]
    fn app_sse_frames_use_eventsource_newline_delimiters() {
        let frame = app_sse_data_frame(&serde_json::json!({"type":"ready"}));
        assert_eq!(frame.as_ref(), b"data: {\"type\":\"ready\"}\n\n");
        assert!(!frame.as_ref().windows(2).any(|window| window == b"\\n"));
    }

    #[test]
    fn sessions_survive_restart_and_use_restrictive_permissions() {
        let directory = tempfile::tempdir().expect("temporary session directory");
        let path = directory.path().join("sessions.json");
        let mut sessions = BTreeMap::new();
        sessions.insert(
            "user-a".to_owned(),
            DaemonSession {
                id: "user-a".to_owned(),
                created_at: 10,
                updated_at: 20,
                title: "Conversation".to_owned(),
                status: default_session_status(),
                messages: Vec::new(),
            },
        );
        persist_daemon_sessions(path.to_str().unwrap(), &sessions).unwrap();
        let restored = load_daemon_sessions(path.to_str().unwrap()).unwrap();
        assert_eq!(restored.get("user-a").unwrap().updated_at, 20);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn connection_readiness_reports_missing_environment_without_values() {
        let status = connection_readiness(
            "test-connection",
            "test-mode",
            &["SUPERAGENT_TEST_ENVIRONMENT_VARIABLE_9F3E"],
        );
        assert!(!status.configured);
        assert_eq!(
            status.missing_env,
            vec!["SUPERAGENT_TEST_ENVIRONMENT_VARIABLE_9F3E"]
        );
    }
}
