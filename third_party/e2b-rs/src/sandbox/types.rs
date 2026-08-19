//! Public sandbox lifecycle types.

use std::collections::BTreeMap;

use crate::api::schema as api_schema;

/// Whether a sandbox is currently running or paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxState {
    /// The sandbox is running.
    Running,
    /// The sandbox is paused (snapshotted).
    Paused,
}

/// Metadata and runtime details about a sandbox.
#[derive(Debug, Clone)]
pub struct SandboxInfo {
    /// Sandbox identifier.
    pub sandbox_id: String,
    /// Template the sandbox was created from.
    pub template_id: String,
    /// Optional template alias/name.
    pub name: Option<String>,
    /// User-provided metadata.
    pub metadata: BTreeMap<String, String>,
    /// When the sandbox started.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// When the sandbox will expire.
    pub end_at: chrono::DateTime<chrono::Utc>,
    /// Running or paused.
    pub state: SandboxState,
    /// vCPU count.
    pub cpu_count: u32,
    /// Memory in MB.
    pub memory_mb: u32,
    /// envd version.
    pub envd_version: String,
    /// Whether internet access was explicitly set.
    pub allow_internet_access: Option<bool>,
    /// Base domain serving this sandbox's traffic.
    pub sandbox_domain: Option<String>,
}

impl SandboxInfo {
    /// Build a public [`SandboxInfo`] from the generated control-plane detail.
    pub(crate) fn from_detail(d: api_schema::SandboxDetail) -> SandboxInfo {
        let state = match d.state {
            api_schema::SandboxState::Running => SandboxState::Running,
            api_schema::SandboxState::Paused => SandboxState::Paused,
        };
        SandboxInfo {
            sandbox_id: d.sandbox_id,
            template_id: d.template_id,
            name: d.alias,
            metadata: d
                .metadata
                .map(|m| m.0.into_iter().collect())
                .unwrap_or_default(),
            started_at: d.started_at,
            end_at: d.end_at,
            state,
            cpu_count: d.cpu_count.0.get(),
            memory_mb: u32::try_from(d.memory_mb.0).unwrap_or(0),
            envd_version: d.envd_version.0,
            allow_internet_access: d.allow_internet_access,
            sandbox_domain: d.domain,
        }
    }
}

/// Metadata for a sandbox snapshot (see [`crate::Sandbox::create_snapshot`]).
#[derive(Debug, Clone)]
pub struct SnapshotInfo {
    /// The snapshot's identifier (also usable as a template id).
    pub snapshot_id: String,
    /// Template names/aliases this snapshot is registered under.
    pub names: Vec<String>,
}

impl SnapshotInfo {
    /// Map the generated wire type to the public one.
    pub(crate) fn from_schema(s: crate::api::schema::SnapshotInfo) -> SnapshotInfo {
        SnapshotInfo {
            snapshot_id: s.snapshot_id,
            names: s.names,
        }
    }
}

/// Point-in-time resource usage for a sandbox (see [`crate::Sandbox::get_metrics`]).
#[derive(Debug, Clone)]
pub struct SandboxMetrics {
    /// When the sample was taken.
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Number of virtual CPUs.
    pub cpu_count: u32,
    /// CPU usage as a percentage (0–100).
    pub cpu_used_pct: f64,
    /// Memory used, in bytes.
    pub mem_used_bytes: u64,
    /// Total memory, in bytes.
    pub mem_total_bytes: u64,
    /// Page-cache memory, in bytes.
    pub mem_cache_bytes: u64,
    /// Disk used, in bytes.
    pub disk_used_bytes: u64,
    /// Total disk, in bytes.
    pub disk_total_bytes: u64,
}

impl SandboxMetrics {
    /// Map the generated wire metric to the public type (clamping negatives to 0).
    pub(crate) fn from_metric(m: crate::api::schema::SandboxMetric) -> SandboxMetrics {
        SandboxMetrics {
            timestamp: m.timestamp,
            cpu_count: u32::try_from(m.cpu_count).unwrap_or(0),
            cpu_used_pct: f64::from(m.cpu_used_pct),
            mem_used_bytes: u64::try_from(m.mem_used).unwrap_or(0),
            mem_total_bytes: u64::try_from(m.mem_total).unwrap_or(0),
            mem_cache_bytes: u64::try_from(m.mem_cache).unwrap_or(0),
            disk_used_bytes: u64::try_from(m.disk_used).unwrap_or(0),
            disk_total_bytes: u64::try_from(m.disk_total).unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_info_maps_from_generated() {
        let raw = crate::api::schema::SnapshotInfo {
            names: vec!["my-snap".to_string()],
            snapshot_id: "snap_1".to_string(),
        };
        let s = SnapshotInfo::from_schema(raw);
        assert_eq!(s.snapshot_id, "snap_1");
        assert_eq!(s.names, vec!["my-snap".to_string()]);
    }

    #[test]
    fn metrics_map_from_generated() {
        let raw = crate::api::schema::SandboxMetric {
            cpu_count: 2,
            cpu_used_pct: 12.5,
            disk_total: 1000,
            disk_used: 100,
            mem_cache: 10,
            mem_total: 2048,
            mem_used: 512,
            timestamp: "2026-06-30T10:00:00Z".parse().expect("ts"),
            timestamp_unix: 1_780_000_000,
        };
        let m = SandboxMetrics::from_metric(raw);
        assert_eq!(m.cpu_count, 2);
        assert_eq!(m.mem_used_bytes, 512);
        assert_eq!(m.disk_total_bytes, 1000);
        assert!((m.cpu_used_pct - 12.5).abs() < f64::EPSILON);
    }

    #[test]
    fn sandbox_info_converts_from_detail_json() {
        // A representative POST /sandboxes / GET /sandboxes/{id} response.
        let json = r#"{
            "sandboxID": "sbx_123",
            "templateID": "base",
            "clientID": "c1",
            "cpuCount": 2,
            "memoryMB": 1024,
            "diskSizeMB": 1024,
            "envdVersion": "0.6.0",
            "state": "running",
            "startedAt": "2026-06-30T10:00:00Z",
            "endAt": "2026-06-30T10:05:00Z",
            "metadata": {"k": "v"},
            "domain": "e2b.app"
        }"#;
        let detail: crate::api::schema::SandboxDetail =
            serde_json::from_str(json).expect("deserialize SandboxDetail");
        let info = SandboxInfo::from_detail(detail);
        assert_eq!(info.sandbox_id, "sbx_123");
        assert_eq!(info.template_id, "base");
        assert_eq!(info.cpu_count, 2);
        assert_eq!(info.memory_mb, 1024);
        assert_eq!(info.envd_version, "0.6.0");
        assert!(matches!(info.state, SandboxState::Running));
        assert_eq!(info.metadata.get("k").map(String::as_str), Some("v"));
        assert_eq!(info.sandbox_domain.as_deref(), Some("e2b.app"));
    }
}
