//! Options for the sandbox lifecycle builders.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::connection_config::ConnectionConfigOpts;
use crate::sandbox::types::SandboxState;

/// Options for [`Sandbox::create`](crate::Sandbox::create).
#[derive(Default)]
pub struct SandboxCreateOpts {
    /// Template id or alias (default `"base"`).
    pub template: Option<String>,
    /// Sandbox lifetime (default 5 minutes).
    pub timeout: Option<Duration>,
    /// Metadata key/values.
    pub metadata: BTreeMap<String, String>,
    /// Environment variables.
    pub envs: BTreeMap<String, String>,
    /// Secure all envd communication (default true).
    pub secure: Option<bool>,
    /// Allow internet access (default true).
    pub allow_internet_access: Option<bool>,
    /// Connection options (api key, domain, debug, ...).
    pub connection: ConnectionConfigOpts,
}

/// Options for [`Sandbox::connect`](crate::Sandbox::connect).
#[derive(Default)]
pub struct SandboxConnectOpts {
    /// Lifetime to set on (re)connect (default 5 minutes).
    pub timeout: Option<Duration>,
    /// Connection options.
    pub connection: ConnectionConfigOpts,
}

/// Options for [`Sandbox::list`](crate::Sandbox::list).
#[derive(Default)]
pub struct SandboxListOpts {
    /// Filter by state (default both running and paused).
    pub states: Option<Vec<SandboxState>>,
    /// Filter by metadata key/values.
    pub metadata: BTreeMap<String, String>,
    /// Page size (default 100).
    pub limit: Option<u32>,
    /// Connection options.
    pub connection: ConnectionConfigOpts,
}

/// Options for [`Sandbox::list_snapshots`](crate::Sandbox::list_snapshots).
#[derive(Default)]
pub struct SnapshotListOpts {
    /// Only list snapshots created from this sandbox id.
    pub sandbox_id: Option<String>,
    /// Maximum number of snapshots per page.
    pub limit: Option<u32>,
    /// Connection configuration (API key, URL, domain, debug).
    pub connection: ConnectionConfigOpts,
}

/// Options for building signed file URLs ([`crate::Sandbox::upload_url`] /
/// [`crate::Sandbox::download_url`]).
#[derive(Default)]
pub struct SandboxUrlOpts {
    /// The sandbox user the URL authorizes (defaults to `user` on older envd).
    pub user: Option<String>,
    /// If set, produce an expiring signature valid for this many seconds.
    pub signature_expiration_secs: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opts_default_is_empty() {
        let o = SandboxCreateOpts::default();
        assert!(o.template.is_none());
        assert!(o.timeout.is_none());
        assert!(o.metadata.is_empty());
        let l = SandboxListOpts::default();
        assert!(l.states.is_none());
        assert!(l.limit.is_none());
    }
}
