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
use xai_grok_superagent::{ApprovalPolicy, SuperagentConfig};

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

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("superagent server listening on {addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
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
