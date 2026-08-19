use serde::{Deserialize, Serialize};

/// Feature switches for the supercharged runtime.
///
/// Keeping the switches explicit makes staged rollout, support diagnostics, and
/// safe fallback behavior possible without silently pretending a feature exists.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityProfile {
    pub memory: bool,
    pub skills: bool,
    pub mcp: bool,
    pub subagents: bool,
    pub scheduler: bool,
    pub messaging: bool,
    pub voice: bool,
    pub browser: bool,
    pub e2b: bool,
    pub approvals: bool,
    pub observability: bool,
}

impl Default for CapabilityProfile {
    fn default() -> Self {
        Self {
            memory: true,
            skills: true,
            mcp: true,
            subagents: true,
            scheduler: true,
            messaging: false,
            voice: false,
            browser: false,
            e2b: true,
            approvals: true,
            observability: true,
        }
    }
}

impl CapabilityProfile {
    pub fn enabled_names(&self) -> Vec<&'static str> {
        [
            (self.memory, "memory"),
            (self.skills, "skills"),
            (self.mcp, "mcp"),
            (self.subagents, "subagents"),
            (self.scheduler, "scheduler"),
            (self.messaging, "messaging"),
            (self.voice, "voice"),
            (self.browser, "browser"),
            (self.e2b, "e2b"),
            (self.approvals, "approvals"),
            (self.observability, "observability"),
        ]
        .into_iter()
        .filter_map(|(enabled, name)| enabled.then_some(name))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::CapabilityProfile;

    #[test]
    fn default_profile_exposes_safe_core_capabilities() {
        let profile = CapabilityProfile::default();
        assert!(profile.memory);
        assert!(profile.e2b);
        assert!(profile.approvals);
        assert!(!profile.messaging);
        assert!(profile.enabled_names().contains(&"subagents"));
    }
}
