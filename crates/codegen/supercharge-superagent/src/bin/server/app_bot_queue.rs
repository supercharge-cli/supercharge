use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fs,
    path::Path,
};

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
    #[serde(default)]
    pub origin_bot_id: Option<String>,
    #[serde(default)]
    pub origin_session_id: Option<String>,
    #[serde(default)]
    pub origin_work_id: Option<String>,
    #[serde(default)]
    pub group_message_id: Option<String>,
    #[serde(default)]
    pub group_reply_to: Option<String>,
    #[serde(default)]
    pub depth: u8,
    pub queued_at: u64,
    #[serde(default)]
    pub started_at: Option<u64>,
}

impl BotWork {
    pub fn session(&self) -> &str {
        if self.session_id.is_empty() {
            &self.chat_id
        } else {
            &self.session_id
        }
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
    #[serde(default)]
    pub group_messages: Vec<super::app_group::GroupMessage>,
    #[serde(default)]
    pub group_requests: BTreeMap<String, String>,
    #[serde(default)]
    pub group_published: HashSet<String>,
    #[serde(default)]
    pub notifications: BTreeMap<String, BotWork>,
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
        supercharge_superagent::atomic_state::write_private(path, &raw)
            .map_err(|error| error.to_string())
    }

    fn update<T>(
        &mut self,
        path: &Path,
        change: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut next = self.clone();
        let result = change(&mut next)?;
        next.save(path)?;
        *self = next;
        Ok(result)
    }

    fn signature(work: &BotWork) -> String {
        use sha2::{Digest, Sha256};
        let value = if work.origin_bot_id.is_none()
            && work.origin_session_id.is_none()
            && work.origin_work_id.is_none()
            && work.group_message_id.is_none()
            && work.group_reply_to.is_none()
            && work.depth == 0
        {
            serde_json::json!([work.session(), work.bot_id, work.prompt, work.job_id])
        } else {
            serde_json::json!([
                work.session(),
                work.bot_id,
                work.prompt,
                work.job_id,
                work.origin_bot_id,
                work.origin_session_id,
                work.origin_work_id,
                work.group_message_id,
                work.group_reply_to,
                work.depth
            ])
        };
        format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
    }

    pub fn enqueue(&mut self, work: BotWork, path: &Path) -> Result<bool, String> {
        if let Some(signature) = self.settled.get(&work.id) {
            return if signature == &Self::signature(&work) {
                Ok(false)
            } else {
                Err("Request ID has already been used".into())
            };
        }
        if let Some(existing) = self
            .queued
            .iter()
            .find(|pending| pending.id == work.id)
            .or_else(|| self.in_flight.get(&work.id))
            .or_else(|| self.completed.get(&work.id).map(|result| &result.work))
            .or_else(|| self.interrupted.get(&work.id).map(|result| &result.work))
        {
            if Self::signature(existing) != Self::signature(&work) {
                return Err("Request ID has already been used for different work".to_owned());
            }
            return Ok(false);
        }
        if self.queued.len() >= 1024
            || self
                .queued
                .iter()
                .filter(|pending| pending.chat_id == work.chat_id)
                .count()
                >= 32
        {
            return Err("Bot queue is full".to_owned());
        }
        self.update(path, |next| {
            next.queued.push_back(work);
            Ok(true)
        })
    }

    pub fn enqueue_group(
        &mut self,
        message: super::app_group::GroupMessage,
        works: Vec<BotWork>,
        path: &Path,
    ) -> Result<bool, String> {
        use sha2::{Digest, Sha256};
        let signature = format!(
            "{:x}",
            Sha256::digest(
                serde_json::json!([
                    message.sender_bot_id,
                    message.sender_session_id,
                    message.text,
                    message.mentions,
                    message.reply_to,
                ])
                .to_string()
                .as_bytes()
            )
        );
        if let Some(previous) = self.group_requests.get(&message.id) {
            return if previous == &signature {
                Ok(false)
            } else {
                Err("Request ID has already been used for a different group message".into())
            };
        }
        if self.queued.len() + works.len() > 1024
            || works.iter().any(|work| {
                self.queued
                    .iter()
                    .filter(|pending| pending.session() == work.session())
                    .count()
                    >= 32
            })
        {
            return Err("Agent queue is full".into());
        }
        self.update(path, |next| {
            next.group_requests.insert(message.id.clone(), signature);
            next.group_messages.push(message);
            next.queued.extend(works);
            Ok(true)
        })
    }

    pub fn published_group_result(&mut self, id: &str, path: &Path) -> Result<(), String> {
        self.update(path, |next| {
            next.group_published.insert(id.to_owned());
            Ok(())
        })
    }

    fn result_delivery(&mut self, result: &BotResult) {
        use super::app_group::GroupMessage;
        let work = &result.work;
        if let Some(message) = work.group_message_id.as_ref() {
            let id = format!("result-{}", work.id);
            if !self.group_messages.iter().any(|message| message.id == id) {
                let text = if result.status == "succeeded" {
                    result.reply.clone()
                } else {
                    format!("Task {}: {}", result.status, result.error)
                };
                self.group_messages.push(GroupMessage {
                    id,
                    sender_bot_id: Some(work.bot_id.clone()),
                    sender_session_id: Some(work.session().to_owned()),
                    text,
                    mentions: Vec::new(),
                    created_at: result.finished_at,
                    reply_to: Some(
                        work.group_reply_to
                            .clone()
                            .unwrap_or_else(|| message.clone()),
                    ),
                    work_id: Some(work.id.clone()),
                    session_id: Some(work.session().to_owned()),
                    kind: if result.status == "succeeded" {
                        "message"
                    } else {
                        "error"
                    }
                    .to_owned(),
                });
            }
        }
        if let (Some(origin_bot), Some(origin_session), Some(_job)) =
            (&work.origin_bot_id, &work.origin_session_id, &work.job_id)
        {
            if origin_bot == &work.bot_id || work.depth > super::app_group::MAX_DELEGATION_DEPTH {
                return;
            }
            if work
                .origin_work_id
                .as_ref()
                .and_then(|id| self.completed.get(id).or_else(|| self.interrupted.get(id)))
                .is_some_and(|origin| origin.status == "stopped" || origin.status == "interrupted")
            {
                return;
            }
            let id = format!("result-{}", work.id);
            let result_text = if result.status == "succeeded" {
                result.reply.clone()
            } else {
                format!("{}: {}", result.status, result.error)
            };
            let bounded = result_text.chars().take(32_000).collect::<String>();
            let prompt = format!(
                "A delegated task has finished. This is an attributed task result, not a new user request.\nAgent: {}\nTask ID: {}\nStatus: {}\n<agent_result>\n{}\n</agent_result>\nUse this result to continue the original user task or inform the user in this same conversation. Do not automatically repeat completed actions or re-delegate the same work. If this task was stopped or interrupted, report that honestly and do not resume it without the user asking.",
                work.bot_id, work.id, result.status, bounded
            );
            let notification = BotWork {
                id: id.clone(),
                chat_id: origin_session.clone(),
                session_id: origin_session.clone(),
                bot_id: origin_bot.clone(),
                prompt,
                job_id: None,
                origin_bot_id: None,
                origin_session_id: None,
                origin_work_id: None,
                group_message_id: work.group_reply_to.clone(),
                group_reply_to: work.group_reply_to.clone(),
                depth: work.depth,
                queued_at: result.finished_at,
                started_at: None,
            };
            if !self.settled.contains_key(&id)
                && !self.queued.iter().any(|pending| pending.id == id)
                && !self.in_flight.contains_key(&id)
            {
                self.notifications.insert(id, notification);
            }
        }
    }

    fn deliver_notifications(&mut self) {
        let mut ready = self.notifications.values().cloned().collect::<Vec<_>>();
        ready.sort_by_key(|work| work.queued_at);
        for work in ready {
            if self.queued.len() >= 1024 {
                break;
            }
            if self
                .queued
                .iter()
                .filter(|pending| pending.session() == work.session())
                .count()
                >= 32
            {
                continue;
            }
            self.notifications.remove(&work.id);
            if !self.settled.contains_key(&work.id)
                && !self.queued.iter().any(|pending| pending.id == work.id)
            {
                self.queued.push_back(work);
            }
        }
    }

    pub fn start_next(
        &mut self,
        active_chats: &HashSet<String>,
        blocked_bots: &HashSet<String>,
        path: &Path,
    ) -> Result<Option<BotWork>, String> {
        if !self.notifications.is_empty() {
            self.update(path, |next| {
                next.deliver_notifications();
                Ok(())
            })?;
        }
        let Some(index) = self.queued.iter().position(|work| {
            !active_chats.contains(work.session())
                && !blocked_bots.contains(&work.bot_id)
                && !self.in_flight.values().any(|running| {
                    running.session() == work.session() || running.bot_id == work.bot_id
                })
        }) else {
            return Ok(None);
        };
        self.update(path, |next| {
            let mut work = next.queued.remove(index).ok_or("Queued work vanished")?;
            work.started_at = Some(super::unix_millis());
            next.in_flight.insert(work.id.clone(), work.clone());
            Ok(Some(work))
        })
    }

    pub fn finish(
        &mut self,
        id: &str,
        status: &str,
        reply: &str,
        error: &str,
        finished_at: u64,
        path: &Path,
    ) -> Result<BotResult, String> {
        self.update(path, |next| {
            let work = next
                .in_flight
                .remove(id)
                .ok_or("Running work was not found")?;
            let result = BotResult {
                work,
                status: status.to_owned(),
                reply: reply.to_owned(),
                error: error.to_owned(),
                finished_at,
            };
            next.settled
                .insert(id.to_owned(), Self::signature(&result.work));
            next.completed.insert(id.to_owned(), result.clone());
            next.result_delivery(&result);
            next.deliver_notifications();
            if next.completed.len() > 2048 {
                let oldest = next
                    .completed
                    .iter()
                    .min_by_key(|(_, result)| result.finished_at)
                    .map(|(id, _)| id.clone());
                if let Some(id) = oldest {
                    next.completed.remove(&id);
                }
            }
            Ok(result)
        })
    }

    pub fn cancel_queued(
        &mut self,
        chat: &str,
        now: u64,
        path: &Path,
    ) -> Result<Vec<BotResult>, String> {
        self.update(path, |next| {
            let mut stopped = Vec::new();
            next.queued.retain(|work| {
                if work.session() != chat {
                    return true;
                }
                stopped.push(BotResult {
                    work: work.clone(),
                    status: "stopped".to_owned(),
                    reply: String::new(),
                    error: "Stopped before starting".to_owned(),
                    finished_at: now,
                });
                false
            });
            for result in &stopped {
                next.settled
                    .insert(result.work.id.clone(), Self::signature(&result.work));
                next.completed
                    .insert(result.work.id.clone(), result.clone());
                next.result_delivery(result);
            }
            next.deliver_notifications();
            Ok(stopped)
        })
    }

    pub fn recover_interrupted(&mut self, path: &Path, now: u64) -> Result<Vec<BotResult>, String> {
        if self.in_flight.is_empty() {
            return Ok(Vec::new());
        }
        self.update(path, |next| {
            let mut interrupted = Vec::new();
            for (_, work) in std::mem::take(&mut next.in_flight) {
                let result = BotResult {
                    work,
                    status: "interrupted".to_owned(),
                    reply: String::new(),
                    error:
                        "The daemon restarted while the turn was active. Review before retrying."
                            .to_owned(),
                    finished_at: now,
                };
                next.interrupted
                    .insert(result.work.id.clone(), result.clone());
                next.result_delivery(&result);
                interrupted.push(result);
            }
            next.deliver_notifications();
            Ok(interrupted)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(id: &str, chat: &str) -> BotWork {
        BotWork {
            id: id.to_owned(),
            chat_id: chat.to_owned(),
            session_id: chat.to_owned(),
            bot_id: chat.to_owned(),
            prompt: id.to_owned(),
            job_id: None,
            origin_bot_id: None,
            origin_session_id: None,
            origin_work_id: None,
            group_message_id: None,
            group_reply_to: None,
            depth: 0,
            queued_at: 1,
            started_at: None,
        }
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
        assert_eq!(
            queue
                .start_next(&HashSet::new(), &HashSet::new(), &path)
                .unwrap()
                .unwrap()
                .id,
            "first"
        );
        assert_eq!(
            queue
                .start_next(&HashSet::from(["one".to_owned()]), &HashSet::new(), &path)
                .unwrap()
                .unwrap()
                .id,
            "other"
        );
        let mut loaded = BotQueue::load(&path).unwrap();
        let interrupted = loaded.recover_interrupted(&path, 5).unwrap();
        assert_eq!(interrupted.len(), 2);
        assert_eq!(loaded.interrupted["first"].status, "interrupted");
        assert_eq!(
            loaded
                .start_next(&HashSet::new(), &HashSet::new(), &path)
                .unwrap()
                .unwrap()
                .id,
            "second"
        );
    }

    #[test]
    fn group_steer_is_persisted_without_scheduling_a_second_turn() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("queue.json");
        let mut queue = BotQueue::default();
        let mut running = work("running", "group-main");
        running.group_message_id = Some("question".into());
        queue.enqueue(running, &path).unwrap();
        queue
            .start_next(&HashSet::new(), &HashSet::new(), &path)
            .unwrap();
        let steer = super::super::app_group::GroupMessage {
            id: "steer-one".into(),
            text: "focus on this".into(),
            mentions: vec!["main".into()],
            reply_to: Some("question".into()),
            kind: "steer".into(),
            ..Default::default()
        };
        assert!(queue.enqueue_group(steer.clone(), Vec::new(), &path).unwrap());
        assert!(!queue.enqueue_group(steer, Vec::new(), &path).unwrap());
        let loaded = BotQueue::load(&path).unwrap();
        assert!(loaded.queued.is_empty());
        assert_eq!(loaded.in_flight.len(), 1);
        assert_eq!(loaded.group_messages.len(), 1);
        assert_eq!(loaded.group_messages[0].kind, "steer");
    }

    #[test]
    fn handoff_blocks_only_its_bot_without_claiming_its_queued_work() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("queue.json");
        let mut queue = BotQueue::default();
        queue.enqueue(work("first", "research"), &path).unwrap();
        queue.enqueue(work("second", "design"), &path).unwrap();
        let blocked = HashSet::from(["research".to_owned()]);
        assert_eq!(queue.start_next(&HashSet::new(), &blocked, &path).unwrap().unwrap().id, "second");
        assert!(queue.start_next(&HashSet::new(), &blocked, &path).unwrap().is_none());
        assert_eq!(queue.queued.front().unwrap().id, "first");
        assert_eq!(queue.start_next(&HashSet::new(), &HashSet::new(), &path).unwrap().unwrap().id, "first");
    }

    #[test]
    fn delegated_completion_enqueues_the_original_conversation_once_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("queue.json");
        let mut queue = BotQueue::default();
        let mut delegated = work("job-one", "worker-dm");
        delegated.bot_id = "worker".into();
        delegated.job_id = Some("job-one".into());
        delegated.origin_bot_id = Some("main".into());
        delegated.origin_session_id = Some("main-original-dm".into());
        delegated.origin_work_id = Some("original-turn".into());
        queue.enqueue(delegated.clone(), &path).unwrap();
        queue.start_next(&HashSet::new(), &HashSet::new(), &path).unwrap();
        queue
            .finish("job-one", "succeeded", "answer from worker", "", 9, &path)
            .unwrap();
        let mut loaded = BotQueue::load(&path).unwrap();
        assert!(!loaded.enqueue(delegated, &path).unwrap());
        assert_eq!(loaded.queued.len(), 1);
        let notification = loaded.start_next(&HashSet::new(), &HashSet::new(), &path).unwrap().unwrap();
        assert_eq!(notification.session(), "main-original-dm");
        assert_eq!(notification.bot_id, "main");
        assert!(notification.prompt.contains("answer from worker"));
        assert!(notification.job_id.is_none());
    }

    #[test]
    fn group_message_and_all_recipients_are_atomic_and_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("queue.json");
        let mut queue = BotQueue::default();
        let message = super::super::app_group::GroupMessage {
            id: "message-one".into(),
            text: "shared only".into(),
            mentions: vec!["alpha".into(), "beta".into()],
            ..Default::default()
        };
        assert!(
            queue
                .enqueue_group(
                    message.clone(),
                    vec![work("one", "group-alpha"), work("two", "group-beta")],
                    &path
                )
                .unwrap()
        );
        assert!(!queue.enqueue_group(message.clone(), vec![], &path).unwrap());
        let loaded = BotQueue::load(&path).unwrap();
        assert_eq!(loaded.queued.len(), 2);
        assert_eq!(loaded.group_messages.len(), 1);
        let mut changed = message;
        changed.text = "different".into();
        assert!(queue.enqueue_group(changed, vec![], &path).is_err());
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
