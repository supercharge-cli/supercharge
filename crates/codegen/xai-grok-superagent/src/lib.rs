//! Hermes-inspired extensions for the grok-build Rust runtime.
//!
//! This crate deliberately keeps feature switches, approval policy, and
//! scheduling independent from the interactive shell. The shell can adopt the
//! pieces incrementally without creating a second agent implementation.

mod approval;
mod capabilities;
pub mod commands;
pub mod controls;
pub mod discord;
mod e2b;
pub mod gateway;
pub mod integrations;
pub mod jobs;
pub mod matrix;
pub mod memory;
mod scheduler;
pub mod signal;
pub mod slack;
pub mod telegram;
pub mod webhook;
pub mod whatsapp;

use sha2::{Digest, Sha256};

pub use approval::{ApprovalError, ApprovalPolicy};
pub use capabilities::CapabilityProfile;
#[cfg(feature = "e2b")]
pub use e2b::E2bExecutor;
pub use e2b::{E2bError, ExecutionResult, PersistentExecutionRequest, SandboxPlan, SandboxStatus};
pub use scheduler::ScheduledJob;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SuperagentConfig {
    #[serde(default)]
    pub capabilities: CapabilityProfile,
    #[serde(default)]
    pub max_tool_calls_per_turn: u32,
    #[serde(default)]
    pub max_subagent_depth: u8,
    #[serde(default)]
    pub max_parallel_subagents: u8,
}

impl Default for SuperagentConfig {
    fn default() -> Self {
        Self {
            capabilities: CapabilityProfile::default(),
            max_tool_calls_per_turn: 32,
            max_subagent_depth: 3,
            max_parallel_subagents: 8,
        }
    }
}

impl SuperagentConfig {
    /// Stable identifier for logs, support diagnostics, and config caching.
    pub fn fingerprint(&self) -> String {
        let encoded = serde_json::to_vec(self).expect("SuperagentConfig is serializable");
        let digest = Sha256::digest(encoded);
        format!("sha256:{digest:x}")
    }
}

#[cfg(test)]
mod tests {
    use super::SuperagentConfig;

    #[test]
    fn default_config_has_stable_fingerprint() {
        let config = SuperagentConfig::default();
        assert_eq!(
            config.fingerprint(),
            SuperagentConfig::default().fingerprint()
        );
        assert!(config.fingerprint().starts_with("sha256:"));
    }
}
