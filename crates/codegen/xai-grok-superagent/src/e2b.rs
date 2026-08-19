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
        use e2b_rs::{CommandStartOpts, Sandbox};

        let mut builder = Sandbox::create()
            .template(self.plan.template.clone())
            .timeout(Duration::from_secs(self.plan.timeout_seconds));
        if let Some(api_key) = &self.api_key {
            builder = builder.api_key(api_key.clone());
        }

        let sandbox = builder
            .await
            .map_err(|error| E2bError::Execution(error.to_string()))?;
        let sandbox_id = sandbox.sandbox_id().to_owned();
        let result = sandbox
            .commands()
            .run(command, CommandStartOpts::default())
            .await
            .map_err(|error| E2bError::Execution(error.to_string()));

        // Teardown is best effort. A sandbox that expired during command
        // execution must not replace the command's actual result.
        let _ = sandbox.kill().await;
        let result = result?;
        Ok(ExecutionResult {
            sandbox_id: Some(sandbox_id),
            exit_code: result.exit_code,
            stdout: cap_output(result.stdout, self.plan.max_output_bytes),
            stderr: cap_output(result.stderr, self.plan.max_output_bytes),
        })
    }
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
    use super::{SandboxPlan, cap_output};

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
}
