use std::{convert::Infallible, env, net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    body::Body,
    extract::{Query, State},
    http::{Request, StatusCode, header},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
#[cfg(feature = "e2b")]
use xai_grok_superagent::SandboxPlan;
use xai_grok_superagent::{
    ApprovalPolicy, SuperagentConfig,
    discord::DiscordClient,
    gateway::GatewayStateStore,
    telegram::{TelegramClient, TelegramConfig},
};

#[derive(Clone)]
struct AppState {
    config: SuperagentConfig,
    approval: ApprovalPolicy,
    api_token: Option<String>,
    #[cfg(feature = "e2b")]
    sandbox_plan: SandboxPlan,
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

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bind = env::var("SUPERAGENT_BIND").unwrap_or_else(|_| "127.0.0.1:8787".to_owned());
    let addr: SocketAddr = bind.parse()?;
    let api_token = env::var("SUPERAGENT_API_TOKEN")
        .ok()
        .filter(|v| !v.is_empty());
    let state = AppState {
        config: SuperagentConfig::default(),
        approval: ApprovalPolicy::default(),
        api_token,
        #[cfg(feature = "e2b")]
        sandbox_plan: SandboxPlan::default(),
    };

    let protected = Router::new()
        .route("/v1/approval/check", get(check_approval))
        .route("/v1/exec", post(execute))
        .with_state(Arc::new(state.clone()))
        .layer(middleware::from_fn_with_state(
            Arc::new(state.clone()),
            auth,
        ));
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/capabilities", get(capabilities))
        .with_state(Arc::new(state.clone()))
        .merge(protected);

    if env::var("DISCORD_BOT_TOKEN").is_ok() {
        tokio::spawn(async {
            if let Err(error) = run_discord_daemon().await {
                eprintln!("discord gateway stopped: {error}");
            }
        });
    }
    if env::var("TELEGRAM_BOT_TOKEN").is_ok() {
        tokio::spawn(async {
            if let Err(error) = run_telegram_daemon().await {
                eprintln!("telegram gateway stopped: {error}");
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("superagent server listening on {addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
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
                    } else if env::var_os("GROK_AGENT_BINARY").is_some() {
                        match run_grok_turn(&message).await {
                            Ok(Some(reply)) if !reply.is_empty() => {
                                send_chunked(&client, message.chat_id, &reply).await?;
                            }
                            Ok(_) => {}
                            Err(error) => {
                                eprintln!("grok headless turn failed: {error}");
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

async fn run_discord_daemon() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = xai_grok_superagent::discord::DiscordConfig::from_env()?;
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
                    if let Some(binary) = env::var_os("GROK_AGENT_BINARY") {
                        match run_grok_turn(&message).await {
                            Ok(Some(reply)) if !reply.is_empty() => {
                                send_discord_chunked(&client, message.chat_id, &reply).await?;
                            }
                            Ok(_) => {}
                            Err(error) => {
                                eprintln!("grok headless Discord turn failed: {error}");
                                let _ = binary;
                                client
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

async fn run_grok_turn(
    message: &xai_grok_superagent::gateway::GatewayMessage,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let binary = env::var("GROK_AGENT_BINARY")?;
    let output = tokio::process::Command::new(binary)
        .arg("--single")
        .arg(&message.text)
        .arg("--session-id")
        .arg(message.session_key().as_str())
        .output()
        .await?;
    if !output.status.success() {
        return Err(format!("grok exited with status {}", output.status).into());
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    ))
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

async fn auth(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(expected) = &state.api_token else {
        return Ok(next.run(request).await);
    };
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if supplied == Some(expected.as_str()) {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
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
        let executor = xai_grok_superagent::E2bExecutor::new(state.sandbox_plan.clone(), api_key);
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
