use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentJob {
    pub id: String,
    pub session_key: String,
    pub platform: String,
    pub chat_id: i64,
    pub prompt: String,
    pub status: JobStatus,
    pub attempts: u32,
    pub max_attempts: u32,
    pub next_attempt_at: u64,
    pub lease_until: Option<u64>,
    pub created_at: u64,
    pub updated_at: u64,
    pub last_error: Option<String>,
    #[serde(default)]
    pub result: Option<String>,
}

#[derive(Debug, Error)]
pub enum JobStoreError {
    #[error("job store I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("job store JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("job not found: {0}")]
    NotFound(String),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedJobs {
    jobs: BTreeMap<String, AgentJob>,
}

pub struct JobStore {
    path: PathBuf,
    state: PersistedJobs,
}

impl JobStore {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, JobStoreError> {
        let path = path.into();
        let state = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            PersistedJobs::default()
        };
        Ok(Self { path, state })
    }

    pub fn enqueue(
        &mut self,
        session_key: impl Into<String>,
        platform: impl Into<String>,
        chat_id: i64,
        prompt: impl Into<String>,
        max_attempts: u32,
    ) -> Result<AgentJob, JobStoreError> {
        let now = now_seconds();
        let job = AgentJob {
            id: Uuid::new_v4().to_string(),
            session_key: session_key.into(),
            platform: platform.into(),
            chat_id,
            prompt: prompt.into(),
            status: JobStatus::Queued,
            attempts: 0,
            max_attempts: max_attempts.max(1),
            next_attempt_at: now,
            lease_until: None,
            created_at: now,
            updated_at: now,
            last_error: None,
            result: None,
        };
        self.state.jobs.insert(job.id.clone(), job.clone());
        self.persist()?;
        Ok(job)
    }

    pub fn recover_expired_leases(&mut self, now: u64) -> Result<usize, JobStoreError> {
        let mut recovered = 0;
        for job in self.state.jobs.values_mut() {
            if job.status == JobStatus::Running && job.lease_until.is_some_and(|lease| lease <= now)
            {
                job.status = JobStatus::Queued;
                job.lease_until = None;
                job.next_attempt_at = now;
                job.updated_at = now;
                recovered += 1;
            }
        }
        if recovered > 0 {
            self.persist()?;
        }
        Ok(recovered)
    }

    pub fn claim_next(
        &mut self,
        now: u64,
        lease_seconds: u64,
    ) -> Result<Option<AgentJob>, JobStoreError> {
        let selected = self
            .state
            .jobs
            .values()
            .filter(|job| job.status == JobStatus::Queued && job.next_attempt_at <= now)
            .min_by_key(|job| (job.created_at, job.id.clone()))
            .map(|job| job.id.clone());
        let Some(id) = selected else {
            return Ok(None);
        };
        let job = self
            .state
            .jobs
            .get_mut(&id)
            .ok_or_else(|| JobStoreError::NotFound(id.clone()))?;
        job.status = JobStatus::Running;
        job.attempts = job.attempts.saturating_add(1);
        job.lease_until = Some(now.saturating_add(lease_seconds.max(1)));
        job.updated_at = now;
        let claimed = job.clone();
        self.persist()?;
        Ok(Some(claimed))
    }

    pub fn complete_with_result(
        &mut self,
        id: &str,
        result: Option<String>,
    ) -> Result<(), JobStoreError> {
        let job = self
            .state
            .jobs
            .get_mut(id)
            .ok_or_else(|| JobStoreError::NotFound(id.to_owned()))?;
        job.status = JobStatus::Succeeded;
        job.lease_until = None;
        job.updated_at = now_seconds();
        job.result = result.map(|mut value| {
            if value.len() > 200_000 {
                value.truncate(200_000);
                value.push_str("\n[output truncated]");
            }
            value
        });
        self.persist()
    }

    pub fn complete(&mut self, id: &str) -> Result<(), JobStoreError> {
        let job = self
            .state
            .jobs
            .get_mut(id)
            .ok_or_else(|| JobStoreError::NotFound(id.to_owned()))?;
        job.status = JobStatus::Succeeded;
        job.lease_until = None;
        job.updated_at = now_seconds();
        self.persist()
    }

    pub fn fail(
        &mut self,
        id: &str,
        error: impl Into<String>,
        retry_delay_seconds: u64,
    ) -> Result<(), JobStoreError> {
        let now = now_seconds();
        let job = self
            .state
            .jobs
            .get_mut(id)
            .ok_or_else(|| JobStoreError::NotFound(id.to_owned()))?;
        job.last_error = Some(error.into());
        job.lease_until = None;
        job.updated_at = now;
        if job.attempts < job.max_attempts {
            job.status = JobStatus::Queued;
            job.next_attempt_at = now.saturating_add(retry_delay_seconds);
        } else {
            job.status = JobStatus::Failed;
        }
        self.persist()
    }

    pub fn get(&self, id: &str) -> Option<&AgentJob> {
        self.state.jobs.get(id)
    }

    pub fn all(&self) -> impl Iterator<Item = &AgentJob> {
        self.state.jobs.values()
    }

    fn persist(&self) -> Result<(), JobStoreError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = self.path.with_extension("tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(&self.state)?)?;
        std::fs::rename(temp, &self.path)?;
        Ok(())
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{JobStatus, JobStore};

    #[test]
    fn jobs_survive_restart_and_recover_expired_leases() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let mut store = JobStore::load(&path).unwrap();
        let job = store
            .enqueue("agent:main:telegram:chat:1", "telegram", 1, "hello", 2)
            .unwrap();
        let claimed = store.claim_next(u64::MAX - 10, 1).unwrap();
        assert_eq!(claimed.unwrap().id, job.id);
        store.recover_expired_leases(u64::MAX).unwrap();
        let restored = JobStore::load(&path).unwrap();
        assert_eq!(restored.get(&job.id).unwrap().status, JobStatus::Queued);
        assert!(restored.get(&job.id).unwrap().result.is_none());
    }

    #[test]
    fn failed_jobs_retry_until_attempt_limit() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let mut store = JobStore::load(&path).unwrap();
        let job = store.enqueue("session", "telegram", 1, "hello", 2).unwrap();
        let claimed = store.claim_next(u64::MAX - 10, 1).unwrap().unwrap();
        store.fail(&claimed.id, "temporary", 0).unwrap();
        assert_eq!(store.get(&job.id).unwrap().status, JobStatus::Queued);
        let claimed = store.claim_next(u64::MAX, 1).unwrap().unwrap();
        store.fail(&claimed.id, "permanent", 0).unwrap();
        assert_eq!(store.get(&job.id).unwrap().status, JobStatus::Failed);
    }
}
