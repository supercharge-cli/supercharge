use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, RwLock, watch};

#[derive(Clone)]
pub struct ActiveHandoff {
    pub id: String,
    pub bot_id: String,
    pub expires_at: u64,
}

struct Ticket {
    handoff_id: String,
    bot_id: String,
    expires: Instant,
}

#[derive(Default)]
pub struct DesktopControl {
    pub active: RwLock<HashMap<String, ActiveHandoff>>,
    tickets: Mutex<HashMap<String, Ticket>>,
    sockets: Mutex<HashMap<String, (String, String, watch::Sender<bool>)>>,
    settled: Notify,
}

impl DesktopControl {
    pub fn shared() -> Arc<Self> {
        static CONTROL: OnceLock<Arc<DesktopControl>> = OnceLock::new();
        CONTROL.get_or_init(|| Arc::new(Self::default())).clone()
    }

    pub async fn activate(
        &self,
        id: &str,
        bot_id: &str,
        expires_at: u64,
    ) -> Result<(), &'static str> {
        let mut active = self.active.write().await;
        if active.get(bot_id).is_some_and(|current| current.id != id) {
            return Err("Another handoff is active for this bot");
        }
        active.insert(bot_id.to_owned(), ActiveHandoff {
            id: id.to_owned(),
            bot_id: bot_id.to_owned(),
            expires_at,
        });
        Ok(())
    }

    pub async fn mint(&self, id: &str, bot_id: &str, now: u64) -> Result<String, &'static str> {
        let active = self.active.read().await;
        if !Self::allows(&active, id, bot_id, now) {
            return Err("No current handoff");
        }
        let mut tickets = self
            .tickets
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        tickets.retain(|_, ticket| ticket.expires > Instant::now());
        if tickets.len() >= 256 {
            return Err("Too many outstanding desktop tickets");
        }
        let ticket = uuid::Uuid::new_v4().to_string();
        tickets.insert(
            ticket.clone(),
            Ticket {
                handoff_id: id.to_owned(),
                bot_id: bot_id.to_owned(),
                expires: Instant::now() + Duration::from_secs(60),
            },
        );
        Ok(ticket)
    }

    pub fn consume(&self, ticket: &str) -> Option<(String, String)> {
        let ticket = self
            .tickets
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(ticket)?;
        (ticket.expires > Instant::now()).then_some((ticket.handoff_id, ticket.bot_id))
    }

    pub fn allows(active: &HashMap<String, ActiveHandoff>, id: &str, bot_id: &str, now: u64) -> bool {
        active.get(bot_id).is_some_and(|current| current.id == id && current.expires_at > now)
    }

    pub async fn register(
        self: &Arc<Self>,
        id: &str,
        bot_id: &str,
        now: u64,
    ) -> Option<(SocketGuard, watch::Receiver<bool>)> {
        let active = self.active.read().await;
        if !Self::allows(&active, id, bot_id, now) {
            return None;
        }
        let (cancel, receiver) = watch::channel(false);
        let key = uuid::Uuid::new_v4().to_string();
        self.sockets
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(key.clone(), (bot_id.to_owned(), id.to_owned(), cancel));
        Some((
            SocketGuard {
                control: self.clone(),
                key,
            },
            receiver,
        ))
    }

    pub async fn revoke(&self, bot_id: &str, id: &str) -> Result<(), &'static str> {
        {
            // Relays hold a read guard while forwarding input. Revocation waits for those sends.
            let mut active = self.active.write().await;
            if active.get(bot_id).is_some_and(|current| current.id == id) {
                active.remove(bot_id);
            }
            self.tickets
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .retain(|_, ticket| ticket.bot_id != bot_id || ticket.handoff_id != id);
            for (socket_bot, handoff, cancel) in self
                .sockets
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .values()
            {
                if socket_bot == bot_id && handoff == id {
                    let _ = cancel.send(true);
                }
            }
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let notified = self.settled.notified();
                if !self
                    .sockets
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .values()
                    .any(|(socket_bot, handoff, _)| socket_bot == bot_id && handoff == id)
                {
                    return;
                }
                notified.await;
            }
        })
        .await
        .map_err(|_| "Desktop connections are still closing; retry returning control")
    }
}

pub struct SocketGuard {
    control: Arc<DesktopControl>,
    key: String,
}
impl Drop for SocketGuard {
    fn drop(&mut self) {
        self.control
            .sockets
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.key);
        self.control.settled.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tickets_are_single_use_and_bound_to_control() {
        let control = DesktopControl::default();
        assert!(control.mint("a", "main", 0).await.is_err());
        control.activate("a", "main", 100).await.unwrap();
        assert!(control.mint("b", "main", 0).await.is_err());
        let ticket = control.mint("a", "main", 0).await.unwrap();
        control.activate("a", "other", 100).await.unwrap();
        assert!(control.activate("different", "main", 100).await.is_err());
        assert!(control.mint("a", "other", 0).await.is_ok());
        assert!(!DesktopControl::allows(
            &*control.active.read().await,
            "different",
            "other",
            0
        ));
        assert_eq!(control.consume(&ticket), Some(("a".into(), "main".into())));
        assert!(control.consume(&ticket).is_none());
        assert!(control.mint("a", "main", 100).await.is_err());
    }

    #[tokio::test]
    async fn simultaneous_bots_with_same_handoff_id_revoke_only_their_own_tickets_and_sockets() {
        let control = Arc::new(DesktopControl::default());
        control.activate("shared", "main", 100).await.unwrap();
        control.activate("shared", "research", 100).await.unwrap();
        let main_ticket = control.mint("shared", "main", 0).await.unwrap();
        let research_ticket = control.mint("shared", "research", 0).await.unwrap();
        let (main_socket, mut main_cancelled) = control.register("shared", "main", 0).await.unwrap();
        let (research_socket, research_cancelled) = control.register("shared", "research", 0).await.unwrap();
        let revoking = tokio::spawn({
            let control = control.clone();
            async move { control.revoke("main", "shared").await }
        });
        main_cancelled.changed().await.unwrap();
        assert!(*main_cancelled.borrow());
        assert!(!research_cancelled.has_changed().unwrap());
        assert!(!revoking.is_finished());
        drop(main_socket);
        revoking.await.unwrap().unwrap();
        assert!(control.consume(&main_ticket).is_none());
        assert_eq!(control.consume(&research_ticket), Some(("shared".into(), "research".into())));
        assert!(control.mint("shared", "research", 0).await.is_ok());
        assert!(!DesktopControl::allows(&*control.active.read().await, "shared", "main", 0));
        assert!(DesktopControl::allows(&*control.active.read().await, "shared", "research", 0));
        control.revoke("main", "shared").await.unwrap();
        assert!(!research_cancelled.has_changed().unwrap());
        drop(research_socket);
    }

    #[tokio::test]
    async fn revocation_waits_for_live_sockets_and_stale_return_cannot_revoke_new_owner() {
        let control = Arc::new(DesktopControl::default());
        control.activate("a", "main", 100).await.unwrap();
        let ticket = control.mint("a", "main", 0).await.unwrap();
        let (guard, mut cancellation) = control.register("a", "main", 0).await.unwrap();
        let pending = tokio::spawn({
            let control = control.clone();
            async move { control.revoke("main", "a").await }
        });
        cancellation.changed().await.unwrap();
        assert!(*cancellation.borrow());
        assert!(!pending.is_finished());
        drop(guard);
        pending.await.unwrap().unwrap();
        assert!(control.consume(&ticket).is_none());
        control.activate("b", "research", 200).await.unwrap();
        control.revoke("main", "a").await.unwrap();
        assert!(control.mint("b", "research", 100).await.is_ok());
    }
}
