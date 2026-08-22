use std::{collections::BTreeMap, path::PathBuf};

use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

use crate::scheduler::ScheduledJob;

#[derive(Debug, Error)]
pub enum ScheduleStoreError {
    #[error("schedule store I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("schedule store JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid schedule: {0}")]
    Invalid(String),
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct PersistedSchedules {
    jobs: BTreeMap<Uuid, ScheduledJob>,
}

#[derive(Debug)]
pub struct ScheduleStore {
    path: PathBuf,
    state: PersistedSchedules,
}

impl ScheduleStore {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, ScheduleStoreError> {
        let path = path.into();
        let state = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            PersistedSchedules::default()
        };
        Ok(Self { path, state })
    }

    pub fn create(
        &mut self,
        name: impl Into<String>,
        prompt: impl Into<String>,
        interval_seconds: u64,
        cron: Option<String>,
        home_channel: Option<String>,
    ) -> Result<ScheduledJob, ScheduleStoreError> {
        let mut job = ScheduledJob::new(name, prompt, interval_seconds)
            .with_cron(cron)
            .map_err(|error| ScheduleStoreError::Invalid(error.to_string()))?;
        job.home_channel = home_channel;
        self.state.jobs.insert(job.id, job.clone());
        self.persist()?;
        Ok(job)
    }

    pub fn list(&self) -> impl Iterator<Item = &ScheduledJob> {
        self.state.jobs.values()
    }

    pub fn get(&self, id: &Uuid) -> Option<&ScheduledJob> {
        self.state.jobs.get(id)
    }

    pub fn delete(&mut self, id: &Uuid) -> Result<bool, ScheduleStoreError> {
        let removed = self.state.jobs.remove(id).is_some();
        if removed {
            self.persist()?;
        }
        Ok(removed)
    }

    pub fn set_enabled(&mut self, id: &Uuid, enabled: bool) -> Result<bool, ScheduleStoreError> {
        let Some(job) = self.state.jobs.get_mut(id) else {
            return Ok(false);
        };
        job.enabled = enabled;
        self.persist()?;
        Ok(true)
    }

    pub fn claim_due(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<Vec<ScheduledJob>, ScheduleStoreError> {
        let mut claimed = Vec::new();
        for job in self.state.jobs.values_mut() {
            if job.claim_next_run(now) {
                claimed.push(job.clone());
            }
        }
        if !claimed.is_empty() {
            self.persist()?;
        }
        Ok(claimed)
    }

    fn persist(&self) -> Result<(), ScheduleStoreError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = self.path.with_extension("tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(&self.state)?)?;
        std::fs::rename(temp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ScheduleStore;
    use chrono::Utc;
    use tempfile::tempdir;

    #[test]
    fn schedules_survive_restart_and_claim_due_jobs_once() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("schedules.json");
        let mut store = ScheduleStore::load(&path).unwrap();
        let job = store
            .create(
                "heartbeat",
                "check status",
                60,
                None,
                Some("telegram:1".to_owned()),
            )
            .unwrap();
        let claimed = store.claim_due(Utc::now()).unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, job.id);
        assert!(ScheduleStore::load(&path).unwrap().get(&job.id).is_some());
        assert!(
            ScheduleStore::load(&path)
                .unwrap()
                .list()
                .next()
                .unwrap()
                .home_channel
                .is_some()
        );
    }
}
