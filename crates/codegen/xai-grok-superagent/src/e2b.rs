#[cfg(feature = "e2b")]
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxPlan {
    pub template: String,
    pub timeout_seconds: u64,
    pub max_output_bytes: usize,
}

impl Default for SandboxPlan {
    fn default() -> Self {
        Self {
            template: "base".to_owned(),
            timeout_seconds: 900,
            max_output_bytes: 200_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionResult {
    pub sandbox_id: Option<String>,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistentExecutionRequest {
    pub sandbox_id: Option<String>,
    pub command: String,
    pub cwd: Option<String>,
    pub workspace_root: String,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxStatus {
    pub sandbox_id: String,
    pub state: String,
    pub template_id: String,
    pub resumable: bool,
    pub cpu_count: u32,
    pub memory_mb: u32,
    pub workspace_root: String,
}

#[derive(Debug, Error)]
pub enum E2bError {
    #[error("E2B support is not compiled in; enable the `e2b` feature")]
    FeatureDisabled,
    #[error("E2B execution failed: {0}")]
    Execution(String),
}

#[cfg(feature = "e2b")]
pub struct E2bExecutor {
    plan: SandboxPlan,
    api_key: Option<String>,
}

#[cfg(feature = "e2b")]
impl E2bExecutor {
    pub fn new(plan: SandboxPlan, api_key: Option<String>) -> Self {
        Self { plan, api_key }
    }

    pub async fn execute(&self, command: &str) -> Result<ExecutionResult, E2bError> {
        let sandbox = self.create_sandbox().await?;
        let sandbox_id = sandbox.sandbox_id().to_owned();
        let result = self
            .run_command(&sandbox, command, None, self.plan.timeout_seconds)
            .await;

        // Preserve the one-shot endpoint contract: teardown is best effort and
        // must not replace the command's actual result.
        let _ = sandbox.kill().await;
        result.map(|mut result| {
            result.sandbox_id = Some(sandbox_id);
            result
        })
    }

    pub async fn ensure_session(
        &self,
        sandbox_id: Option<&str>,
        workspace_root: String,
    ) -> Result<SandboxStatus, E2bError> {
        validate_workspace_path(&workspace_root, None)?;
        let sandbox = if let Some(sandbox_id) = sandbox_id {
            self.connect_sandbox(sandbox_id).await?
        } else {
            self.create_sandbox().await?
        };
        let _ = sandbox
            .set_timeout(Duration::from_secs(self.plan.timeout_seconds))
            .await;
        let info = sandbox
            .get_info()
            .await
            .map_err(|error| E2bError::Execution(error.to_string()))?;
        let state = format!("{:?}", info.state).to_lowercase();
        Ok(SandboxStatus {
            sandbox_id: info.sandbox_id,
            state: state.clone(),
            template_id: info.template_id,
            resumable: matches!(state.as_str(), "running" | "paused"),
            cpu_count: info.cpu_count,
            memory_mb: info.memory_mb,
            workspace_root,
        })
    }

    pub async fn execute_persistent(
        &self,
        request: PersistentExecutionRequest,
    ) -> Result<ExecutionResult, E2bError> {
        validate_workspace_path(&request.workspace_root, request.cwd.as_deref())?;
        let sandbox = if let Some(sandbox_id) = request.sandbox_id.as_deref() {
            self.connect_sandbox(sandbox_id).await?
        } else {
            self.create_sandbox().await?
        };
        let timeout = request
            .timeout_seconds
            .unwrap_or(self.plan.timeout_seconds)
            .clamp(1, 86_400);
        let mut result = self
            .run_command(&sandbox, &request.command, request.cwd.as_deref(), timeout)
            .await?;
        let _ = sandbox
            .set_timeout(Duration::from_secs(self.plan.timeout_seconds))
            .await;
        result.sandbox_id = Some(sandbox.sandbox_id().to_owned());
        Ok(result)
    }

    pub async fn status(
        &self,
        sandbox_id: &str,
        workspace_root: String,
    ) -> Result<SandboxStatus, E2bError> {
        let sandbox = self.connect_sandbox(sandbox_id).await?;
        let info = sandbox
            .get_info()
            .await
            .map_err(|error| E2bError::Execution(error.to_string()))?;
        let state = format!("{:?}", info.state).to_lowercase();
        Ok(SandboxStatus {
            sandbox_id: info.sandbox_id,
            state: state.clone(),
            template_id: info.template_id,
            resumable: matches!(state.as_str(), "running" | "paused"),
            cpu_count: info.cpu_count,
            memory_mb: info.memory_mb,
            workspace_root,
        })
    }

    pub async fn pause(&self, sandbox_id: &str) -> Result<bool, E2bError> {
        let sandbox = self.connect_sandbox(sandbox_id).await?;
        sandbox
            .pause()
            .await
            .map_err(|error| E2bError::Execution(error.to_string()))
    }

    pub async fn kill(&self, sandbox_id: &str) -> Result<bool, E2bError> {
        let sandbox = self.connect_sandbox(sandbox_id).await?;
        sandbox
            .kill()
            .await
            .map_err(|error| E2bError::Execution(error.to_string()))
    }

    async fn create_sandbox(&self) -> Result<e2b_rs::Sandbox, E2bError> {
        let mut builder = e2b_rs::Sandbox::create()
            .template(self.plan.template.clone())
            .timeout(Duration::from_secs(self.plan.timeout_seconds));
        if let Some(api_key) = &self.api_key {
            builder = builder.api_key(api_key.clone());
        }
        builder
            .await
            .map_err(|error| E2bError::Execution(error.to_string()))
    }

    async fn connect_sandbox(&self, sandbox_id: &str) -> Result<e2b_rs::Sandbox, E2bError> {
        let mut builder = e2b_rs::Sandbox::connect(sandbox_id.to_owned())
            .timeout(Duration::from_secs(self.plan.timeout_seconds));
        if let Some(api_key) = &self.api_key {
            builder = builder.api_key(api_key.clone());
        }
        builder
            .await
            .map_err(|error| E2bError::Execution(error.to_string()))
    }

    async fn run_command(
        &self,
        sandbox: &e2b_rs::Sandbox,
        command: &str,
        cwd: Option<&str>,
        timeout_seconds: u64,
    ) -> Result<ExecutionResult, E2bError> {
        use e2b_rs::CommandStartOpts;
        let result = tokio::time::timeout(
            Duration::from_secs(timeout_seconds),
            sandbox.commands().run(
                command,
                CommandStartOpts {
                    cwd: cwd.map(ToOwned::to_owned),
                    ..CommandStartOpts::default()
                },
            ),
        )
        .await
        .map_err(|_| E2bError::Execution(format!("command timed out after {timeout_seconds}s")))?
        .map_err(|error| E2bError::Execution(error.to_string()))?;
        Ok(ExecutionResult {
            sandbox_id: None,
            exit_code: result.exit_code,
            stdout: cap_output(result.stdout, self.plan.max_output_bytes),
            stderr: cap_output(result.stderr, self.plan.max_output_bytes),
        })
    }
}

#[cfg(any(feature = "e2b", test))]
fn validate_workspace_path(workspace_root: &str, cwd: Option<&str>) -> Result<(), E2bError> {
    let root = std::path::Path::new(workspace_root);
    if workspace_root.trim().is_empty() || !root.is_absolute() {
        return Err(E2bError::Execution(
            "workspace_root must be a non-empty absolute path".to_owned(),
        ));
    }
    if let Some(cwd) = cwd {
        let path = std::path::Path::new(cwd);
        if path.is_absolute() && !path.starts_with(root) {
            return Err(E2bError::Execution(
                "cwd is outside the configured sandbox workspace".to_owned(),
            ));
        }
        if cwd.split('/').any(|part| part == "..") {
            return Err(E2bError::Execution(
                "cwd path traversal is not allowed".to_owned(),
            ));
        }
    }
    Ok(())
}

#[cfg(any(feature = "e2b", test))]
fn cap_output(value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let mut capped = value;
    capped.truncate(limit);
    capped.push_str("\n[output truncated]");
    capped
}

#[cfg(test)]
mod tests {
    use super::{SandboxPlan, cap_output, validate_workspace_path};

    #[test]
    fn output_is_capped_with_a_marker() {
        let output = cap_output("abcdef".to_owned(), 3);
        assert_eq!(output, "abc\n[output truncated]");
    }

    #[test]
    fn plan_defaults_are_bounded() {
        let plan = SandboxPlan::default();
        assert_eq!(plan.template, "base");
        assert!(plan.timeout_seconds <= 86_400);
        assert!(plan.max_output_bytes > 0);
    }

    #[test]
    fn workspace_validation_rejects_escape_and_accepts_rooted_cwd() {
        validate_workspace_path("/home/user/workspace", Some("/home/user/workspace/src")).unwrap();
        assert!(validate_workspace_path("/home/user/workspace", Some("/tmp")).is_err());
        assert!(validate_workspace_path("/home/user/workspace", Some("../etc")).is_err());
    }
}
