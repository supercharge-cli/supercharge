use std::{collections::{BTreeMap, HashSet, VecDeque}, fs, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotWork {
    pub id: String,
    pub chat_id: String,
    #[serde(default)]
    pub session_id: String,
    pub bot_id: String,
    pub prompt: String,
    #[serde(default)]
    pub job_id: Option<String>,
    pub queued_at: u64,
    #[serde(default)]
    pub started_at: Option<u64>,
}

impl BotWork {
    pub fn session(&self) -> &str {
        if self.session_id.is_empty() { &self.chat_id } else { &self.session_id }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotResult {
    pub work: BotWork,
    pub status: String,
    pub reply: String,
    pub error: String,
    pub finished_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BotQueue {
    pub queued: VecDeque<BotWork>,
    pub in_flight: BTreeMap<String, BotWork>,
    #[serde(default)]
    pub completed: BTreeMap<String, BotResult>,
    #[serde(default)]
    pub interrupted: BTreeMap<String, BotResult>,
    #[serde(default)]
    settled: BTreeMap<String, String>,
}

impl BotQueue {
    pub fn load(path: &Path) -> Result<Self, String> {
        match fs::read(path) {
            Ok(raw) => serde_json::from_slice(&raw).map_err(|error| error.to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.to_string()),
        }
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        let raw = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        supercharge_superagent::atomic_state::write_private(path, &raw).map_err(|error| error.to_string())
    }

    fn update<T>(&mut self, path: &Path, change: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<T, String> {
        let mut next = self.clone();
        let result = change(&mut next)?;
        next.save(path)?;
        *self = next;
        Ok(result)
    }

    fn signature(work: &BotWork) -> String {
        use sha2::{Digest, Sha256};
        let value = serde_json::json!([work.session(), work.bot_id, work.prompt, work.job_id]);
        format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
    }

    pub fn enqueue(&mut self, work: BotWork, path: &Path) -> Result<bool, String> {
        if let Some(signature) = self.settled.get(&work.id) {
            return if signature == &Self::signature(&work) { Ok(false) } else { Err("Request ID has already been used".into()) };
        }
        if let Some(existing) = self.queued.iter().find(|pending| pending.id == work.id)
            .or_else(|| self.in_flight.get(&work.id))
            .or_else(|| self.completed.get(&work.id).map(|result| &result.work))
            .or_else(|| self.interrupted.get(&work.id).map(|result| &result.work)) {
            if existing.chat_id != work.chat_id || existing.bot_id != work.bot_id || existing.prompt != work.prompt {
                return Err("Request ID has already been used for different work".to_owned());
            }
            return Ok(false);
        }
        if self.queued.len() >= 1024 || self.queued.iter().filter(|pending| pending.chat_id == work.chat_id).count() >= 32 {
            return Err("Bot queue is full".to_owned());
        }
        self.update(path, |next| { next.queued.push_back(work); Ok(true) })
    }

    pub fn start_next(&mut self, active_chats: &HashSet<String>, path: &Path) -> Result<Option<BotWork>, String> {
        let Some(index) = self.queued.iter().position(|work| !active_chats.contains(work.session())
            && !self.in_flight.values().any(|running| running.session() == work.session() || running.bot_id == work.bot_id)) else { return Ok(None); };
        self.update(path, |next| {
            let mut work = next.queued.remove(index).ok_or("Queued work vanished")?;
            work.started_at = Some(super::unix_millis());
            next.in_flight.insert(work.id.clone(), work.clone());
            Ok(Some(work))
        })
    }

    pub fn finish(&mut self, id: &str, status: &str, reply: &str, error: &str, finished_at: u64, path: &Path) -> Result<BotResult, String> {
        self.update(path, |next| {
            let work = next.in_flight.remove(id).ok_or("Running work was not found")?;
            let result = BotResult { work, status: status.to_owned(), reply: reply.to_owned(),
                error: error.to_owned(), finished_at };
            next.settled.insert(id.to_owned(), Self::signature(&result.work));
            next.completed.insert(id.to_owned(), result.clone());
            if next.completed.len() > 2048 {
                let oldest = next.completed.iter().min_by_key(|(_, result)| result.finished_at).map(|(id, _)| id.clone());
                if let Some(id) = oldest { next.completed.remove(&id); }
            }
            Ok(result)
        })
    }

    pub fn cancel_queued(&mut self, chat: &str, now: u64, path: &Path) -> Result<Vec<BotResult>, String> {
        self.update(path, |next| {
            let mut stopped = Vec::new();
            next.queued.retain(|work| {
                if work.session() != chat { return true; }
                stopped.push(BotResult { work: work.clone(), status: "stopped".to_owned(),
                    reply: String::new(), error: "Stopped before starting".to_owned(), finished_at: now });
                false
            });
            for result in &stopped { next.completed.insert(result.work.id.clone(), result.clone()); }
            Ok(stopped)
        })
    }

    pub fn recover_interrupted(&mut self, path: &Path, now: u64) -> Result<Vec<BotResult>, String> {
        if self.in_flight.is_empty() { return Ok(Vec::new()); }
        self.update(path, |next| {
            let mut interrupted = Vec::new();
            for (_, work) in std::mem::take(&mut next.in_flight) {
                let result = BotResult { work, status: "interrupted".to_owned(), reply: String::new(),
                    error: "The daemon restarted while the turn was active. Review before retrying.".to_owned(), finished_at: now };
                next.interrupted.insert(result.work.id.clone(), result.clone());
                interrupted.push(result);
            }
            Ok(interrupted)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(id: &str, chat: &str) -> BotWork {
        BotWork { id: id.to_owned(), chat_id: chat.to_owned(), session_id: chat.to_owned(), bot_id: chat.to_owned(),
            prompt: id.to_owned(), job_id: None, queued_at: 1, started_at: None }
    }

    #[test]
    fn persisted_queue_preserves_fifo_and_never_replays_inflight() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("queue.json");
        let mut queue = BotQueue::default();
        assert!(queue.enqueue(work("first", "one"), &path).unwrap());
        assert!(!queue.enqueue(work("first", "one"), &path).unwrap());
        queue.enqueue(work("second", "one"), &path).unwrap();
        queue.enqueue(work("other", "two"), &path).unwrap();
        assert_eq!(queue.start_next(&HashSet::new(), &path).unwrap().unwrap().id, "first");
        assert_eq!(queue.start_next(&HashSet::from(["one".to_owned()]), &path).unwrap().unwrap().id, "other");
        let mut loaded = BotQueue::load(&path).unwrap();
        let interrupted = loaded.recover_interrupted(&path, 5).unwrap();
        assert_eq!(interrupted.len(), 2);
        assert_eq!(loaded.interrupted["first"].status, "interrupted");
        assert_eq!(loaded.start_next(&HashSet::new(), &path).unwrap().unwrap().id, "second");
    }

    #[test]
    fn cancelling_queued_work_does_not_reschedule_it() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("queue.json");
        let mut queue = BotQueue::default();
        queue.enqueue(work("one", "chat"), &path).unwrap();
        queue.enqueue(work("two", "other"), &path).unwrap();
        assert_eq!(queue.cancel_queued("chat", 9, &path).unwrap().len(), 1);
        assert_eq!(BotQueue::load(&path).unwrap().queued.len(), 1);
    }
}
