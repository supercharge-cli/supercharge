use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduledJob {
    pub id: Uuid,
    pub name: String,
    pub prompt: String,
    pub interval_seconds: u64,
    pub enabled: bool,
    pub next_run_at: DateTime<Utc>,
}

impl ScheduledJob {
    pub fn new(name: impl Into<String>, prompt: impl Into<String>, interval_seconds: u64) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            prompt: prompt.into(),
            interval_seconds: interval_seconds.max(1),
            enabled: true,
            next_run_at: now,
        }
    }

    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.enabled && self.next_run_at <= now
    }

    pub fn claim_next_run(&mut self, now: DateTime<Utc>) -> bool {
        if !self.is_due(now) {
            return false;
        }
        self.next_run_at = now + Duration::seconds(self.interval_seconds as i64);
        true
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::ScheduledJob;

    #[test]
    fn due_job_is_claimed_once_and_moved_forward() {
        let mut job = ScheduledJob::new("daily", "inspect workspace", 60);
        let now = Utc::now() + Duration::seconds(1);
        assert!(job.claim_next_run(now));
        assert!(!job.is_due(now));
    }
}
