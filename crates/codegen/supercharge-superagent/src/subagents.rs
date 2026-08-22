use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{process::Command, sync::Semaphore, time::timeout};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentRequest {
    pub parent_session: String,
    pub prompt: String,
    #[serde(default)]
    pub depth: u8,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
}

fn default_timeout_seconds() -> u64 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentResult {
    pub subagent_id: String,
    pub session_id: String,
    pub exit_code: i32,
    pub output: String,
}

#[derive(Debug, Error)]
pub enum SubagentError {
    #[error("subagents are not configured")]
    NotConfigured,
    #[error("subagent depth {depth} exceeds maximum {max}")]
    DepthExceeded { depth: u8, max: u8 },
    #[error("subagent prompt is empty or too large")]
    InvalidPrompt,
    #[error("subagent concurrency limit reached")]
    Busy,
    #[error("subagent process failed: {0}")]
    Process(#[from] std::io::Error),
    #[error("subagent timed out after {0}s")]
    Timeout(u64),
}

#[derive(Debug)]
pub struct SubagentCoordinator {
    binary: String,
    max_depth: u8,
    max_output_bytes: usize,
    permits: Arc<Semaphore>,
}

static GLOBAL: OnceLock<Arc<SubagentCoordinator>> = OnceLock::new();

impl SubagentCoordinator {
    pub fn new(binary: impl Into<String>, max_depth: u8, max_parallel: usize) -> Self {
        Self {
            binary: binary.into(),
            max_depth,
            max_output_bytes: 200_000,
            permits: Arc::new(Semaphore::new(max_parallel.max(1))),
        }
    }

    pub async fn run(&self, request: SubagentRequest) -> Result<SubagentResult, SubagentError> {
        if request.prompt.trim().is_empty() || request.prompt.len() > 100_000 {
            return Err(SubagentError::InvalidPrompt);
        }
        if request.depth >= self.max_depth {
            return Err(SubagentError::DepthExceeded {
                depth: request.depth,
                max: self.max_depth,
            });
        }
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| SubagentError::Busy)?;
        let subagent_id = Uuid::new_v4().to_string();
        let session_id = format!("{}:subagent:{}", request.parent_session, subagent_id);
        let process = Command::new(&self.binary)
            .arg("--single")
            .arg(&request.prompt)
            .arg("--session-id")
            .arg(&session_id)
            .env(
                "SUPERAGENT_SUBAGENT_DEPTH",
                request.depth.saturating_add(1).to_string(),
            )
            .output();
        let output = timeout(
            Duration::from_secs(request.timeout_seconds.clamp(1, 86_400)),
            process,
        )
        .await
        .map_err(|_| SubagentError::Timeout(request.timeout_seconds))??;
        drop(permit);
        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let mut combined = if stdout.is_empty() { stderr } else { stdout };
        if combined.len() > self.max_output_bytes {
            combined.truncate(self.max_output_bytes);
            combined.push_str("\n[output truncated]");
        }
        Ok(SubagentResult {
            subagent_id,
            session_id,
            exit_code,
            output: combined,
        })
    }
}

pub fn init_global(binary: impl Into<String>, max_depth: u8, max_parallel: usize) {
    let _ = GLOBAL.set(Arc::new(SubagentCoordinator::new(
        binary,
        max_depth,
        max_parallel,
    )));
}

pub fn global() -> Option<Arc<SubagentCoordinator>> {
    GLOBAL.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::{SubagentCoordinator, SubagentError, SubagentRequest};

    #[tokio::test]
    async fn rejects_depth_and_empty_prompts_before_process_spawn() {
        let coordinator = SubagentCoordinator::new("missing-binary", 2, 1);
        let error = coordinator
            .run(SubagentRequest {
                parent_session: "s".to_owned(),
                prompt: "".to_owned(),
                depth: 0,
                timeout_seconds: 1,
            })
            .await
            .unwrap_err();
        assert!(matches!(error, SubagentError::InvalidPrompt));
        let error = coordinator
            .run(SubagentRequest {
                parent_session: "s".to_owned(),
                prompt: "x".to_owned(),
                depth: 2,
                timeout_seconds: 1,
            })
            .await
            .unwrap_err();
        assert!(matches!(error, SubagentError::DepthExceeded { .. }));
    }
}
