use std::collections::HashSet;

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ApprovalError {
    #[error("command is empty")]
    Empty,
    #[error("command contains a blocked executable: {0}")]
    BlockedExecutable(String),
    #[error("command requires explicit approval")]
    RequiresApproval,
}

#[derive(Debug, Clone)]
pub struct ApprovalPolicy {
    blocked_executables: HashSet<String>,
    require_approval_for_network: bool,
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            blocked_executables: [
                "rm", "mkfs", "mount", "umount", "shutdown", "reboot", "poweroff", "sudo",
                "systemctl", "iptables", "nft",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            require_approval_for_network: true,
        }
    }
}

impl ApprovalPolicy {
    pub fn validate(&self, command: &str, approved: bool) -> Result<(), ApprovalError> {
        let mut tokens = command.split_whitespace();
        let executable = tokens.next().ok_or(ApprovalError::Empty)?;
        let executable = executable.rsplit('/').next().unwrap_or(executable).to_ascii_lowercase();
        if self.blocked_executables.contains(&executable) {
            return Err(ApprovalError::BlockedExecutable(executable));
        }

        let has_network_action = command.contains("curl ")
            || command.contains("wget ")
            || command.contains("ssh ")
            || command.contains("scp ")
            || command.contains("http://")
            || command.contains("https://");
        if self.require_approval_for_network && has_network_action && !approved {
            return Err(ApprovalError::RequiresApproval);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ApprovalError, ApprovalPolicy};

    #[test]
    fn blocks_destructive_commands() {
        assert_eq!(
            ApprovalPolicy::default().validate("rm -rf /", true),
            Err(ApprovalError::BlockedExecutable("rm".into()))
        );
    }

    #[test]
    fn requires_approval_for_network_access() {
        assert_eq!(
            ApprovalPolicy::default().validate("curl https://example.com", false),
            Err(ApprovalError::RequiresApproval)
        );
        assert!(ApprovalPolicy::default()
            .validate("curl https://example.com", true)
            .is_ok());
    }
}
