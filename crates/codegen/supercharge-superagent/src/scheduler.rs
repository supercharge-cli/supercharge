use std::{collections::BTreeSet, fmt};

use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduledJob {
    pub id: Uuid,
    pub name: String,
    pub prompt: String,
    pub interval_seconds: u64,
    #[serde(default)]
    pub cron: Option<String>,
    pub enabled: bool,
    #[serde(default)]
    pub home_channel: Option<String>,
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
            cron: None,
            enabled: true,
            home_channel: None,
            next_run_at: now,
        }
    }

    pub fn with_cron(mut self, expression: Option<String>) -> Result<Self, CronError> {
        if let Some(expression) = expression.as_deref() {
            CronSchedule::parse(expression)?;
            self.cron = Some(expression.trim().to_owned());
        }
        Ok(self)
    }

    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.enabled && self.next_run_at <= now
    }

    pub fn claim_next_run(&mut self, now: DateTime<Utc>) -> bool {
        if !self.is_due(now) {
            return false;
        }
        self.next_run_at = match self.cron.as_deref() {
            Some(expression) => CronSchedule::parse(expression)
                .ok()
                .and_then(|schedule| schedule.next_after(now)),
            None => Some(now + Duration::seconds(self.interval_seconds as i64)),
        }
        .unwrap_or_else(|| now + Duration::seconds(self.interval_seconds as i64));
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronError(String);

impl fmt::Display for CronError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CronError {}

#[derive(Debug, Clone)]
struct CronSchedule {
    minute: CronField,
    hour: CronField,
    day_of_month: CronField,
    month: CronField,
    day_of_week: CronField,
}

#[derive(Debug, Clone)]
struct CronField {
    values: BTreeSet<u32>,
}

impl CronSchedule {
    fn parse(expression: &str) -> Result<Self, CronError> {
        let fields = expression.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 5 {
            return Err(CronError(
                "cron expression must contain five fields".to_owned(),
            ));
        }
        Ok(Self {
            minute: CronField::parse(fields[0], 0, 59)?,
            hour: CronField::parse(fields[1], 0, 23)?,
            day_of_month: CronField::parse(fields[2], 1, 31)?,
            month: CronField::parse(fields[3], 1, 12)?,
            day_of_week: CronField::parse(fields[4], 0, 7)?,
        })
    }

    fn matches(&self, value: DateTime<Utc>) -> bool {
        self.minute.values.contains(&value.minute())
            && self.hour.values.contains(&value.hour())
            && self.day_of_month.values.contains(&value.day())
            && self.month.values.contains(&value.month())
            && self
                .day_of_week
                .values
                .contains(&value.weekday().num_days_from_sunday())
    }

    fn next_after(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let mut candidate = now.with_second(0)?.with_nanosecond(0)? + Duration::minutes(1);
        let limit = candidate + Duration::days(366);
        while candidate <= limit {
            if self.matches(candidate) {
                return Some(candidate);
            }
            candidate += Duration::minutes(1);
        }
        None
    }
}

impl CronField {
    fn parse(value: &str, minimum: u32, maximum: u32) -> Result<Self, CronError> {
        let mut values = BTreeSet::new();
        for item in value.split(',') {
            let item = item.trim();
            if item.is_empty() {
                return Err(CronError("cron field contains an empty item".to_owned()));
            }
            let (range, step) = match item.split_once('/') {
                Some((range, step)) => (
                    range,
                    step.parse::<u32>()
                        .map_err(|_| CronError(format!("invalid cron step: {step}")))?,
                ),
                None => (item, 1),
            };
            if step == 0 {
                return Err(CronError("cron step must be positive".to_owned()));
            }
            let (start, end) = match range {
                "*" => (minimum, maximum),
                value if value.contains('-') => {
                    let (start, end) = value
                        .split_once('-')
                        .ok_or_else(|| CronError("invalid cron range".to_owned()))?;
                    (
                        parse_number(start, minimum, maximum)?,
                        parse_number(end, minimum, maximum)?,
                    )
                }
                value => {
                    let number = parse_number(value, minimum, maximum)?;
                    (number, number)
                }
            };
            if start > end {
                return Err(CronError("cron range start exceeds end".to_owned()));
            }
            values.extend((start..=end).step_by(step as usize));
        }
        if values.is_empty() {
            return Err(CronError("cron field contains no values".to_owned()));
        }
        Ok(Self { values })
    }
}

fn parse_number(value: &str, minimum: u32, maximum: u32) -> Result<u32, CronError> {
    let number = value
        .parse::<u32>()
        .map_err(|_| CronError(format!("invalid cron value: {value}")))?;
    if number < minimum || number > maximum {
        return Err(CronError(format!("cron value outside range: {number}")));
    }
    Ok(number)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone, Timelike, Utc};

    use super::ScheduledJob;

    #[test]
    fn due_job_is_claimed_once_and_moved_forward() {
        let mut job = ScheduledJob::new("daily", "inspect workspace", 60);
        let now = Utc::now() + Duration::seconds(1);
        assert!(job.claim_next_run(now));
        assert!(!job.is_due(now));
    }

    #[test]
    fn cron_job_moves_to_next_matching_minute() {
        let mut job = ScheduledJob::new("hourly", "inspect workspace", 60)
            .with_cron(Some("5 * * * *".to_owned()))
            .unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 19, 10, 6, 0).unwrap();
        job.next_run_at = now;
        assert!(job.claim_next_run(now));
        assert_eq!(job.next_run_at.minute(), 5);
        assert_eq!(job.next_run_at.hour(), 11);
    }

    #[test]
    fn invalid_cron_is_rejected() {
        assert!(
            ScheduledJob::new("bad", "prompt", 60)
                .with_cron(Some("every hour".to_owned()))
                .is_err()
        );
    }
}
