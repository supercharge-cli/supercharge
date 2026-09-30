use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, RwLock, watch};

#[derive(Clone)]
pub struct ActiveHandoff {
    pub id: String,
    pub expires_at: u64,
}

struct Ticket {
    handoff_id: String,
    expires: Instant,
}

#[derive(Default)]
pub struct DesktopControl {
    pub active: RwLock<Option<ActiveHandoff>>,
    tickets: Mutex<HashMap<String, Ticket>>,
    sockets: Mutex<HashMap<String, (String, watch::Sender<bool>)>>,
    settled: Notify,
}

impl DesktopControl {
    pub fn shared() -> Arc<Self> {
        static CONTROL: OnceLock<Arc<DesktopControl>> = OnceLock::new();
        CONTROL.get_or_init(|| Arc::new(Self::default())).clone()
    }

    pub async fn activate(&self, id: &str, expires_at: u64) -> Result<(), &'static str> {
        let mut active = self.active.write().await;
        if active.as_ref().is_some_and(|current| current.id != id) { return Err("Another handoff is active"); }
        *active = Some(ActiveHandoff { id: id.to_owned(), expires_at });
        Ok(())
    }

    pub async fn mint(&self, id: &str, now: u64) -> Result<String, &'static str> {
        let active = self.active.read().await;
        if !Self::allows(&active, id, now) { return Err("No current handoff"); }
        let mut tickets = self.tickets.lock().unwrap_or_else(|error| error.into_inner());
        tickets.retain(|_, ticket| ticket.expires > Instant::now());
        if tickets.len() >= 256 { return Err("Too many outstanding desktop tickets"); }
        let ticket = uuid::Uuid::new_v4().to_string();
        tickets.insert(ticket.clone(), Ticket { handoff_id: id.to_owned(), expires: Instant::now() + Duration::from_secs(60) });
        Ok(ticket)
    }

    pub fn consume(&self, ticket: &str) -> Option<String> {
        let ticket = self.tickets.lock().unwrap_or_else(|error| error.into_inner()).remove(ticket)?;
        (ticket.expires > Instant::now()).then_some(ticket.handoff_id)
    }

    pub fn allows(active: &Option<ActiveHandoff>, id: &str, now: u64) -> bool {
        active.as_ref().is_some_and(|current| current.id == id && current.expires_at > now)
    }

    pub async fn register(self: &Arc<Self>, id: &str, now: u64) -> Option<(SocketGuard, watch::Receiver<bool>)> {
        let active = self.active.read().await;
        if !Self::allows(&active, id, now) { return None; }
        let (cancel, receiver) = watch::channel(false);
        let key = uuid::Uuid::new_v4().to_string();
        self.sockets.lock().unwrap_or_else(|error| error.into_inner()).insert(key.clone(), (id.to_owned(), cancel));
        Some((SocketGuard { control: self.clone(), key }, receiver))
    }

    pub async fn revoke(&self, id: &str) -> Result<(), &'static str> {
        {
            // Relays hold a read guard while forwarding input. Revocation waits for those sends.
            let mut active = self.active.write().await;
            if active.as_ref().is_some_and(|current| current.id == id) { *active = None; }
            self.tickets.lock().unwrap_or_else(|error| error.into_inner()).retain(|_, ticket| ticket.handoff_id != id);
            for (handoff, cancel) in self.sockets.lock().unwrap_or_else(|error| error.into_inner()).values() {
                if handoff == id { let _ = cancel.send(true); }
            }
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let notified = self.settled.notified();
                if !self.sockets.lock().unwrap_or_else(|error| error.into_inner()).values().any(|(handoff, _)| handoff == id) { return; }
                notified.await;
            }
        }).await.map_err(|_| "Desktop connections are still closing; retry returning control")
    }
}

pub struct SocketGuard { control: Arc<DesktopControl>, key: String }
impl Drop for SocketGuard {
    fn drop(&mut self) {
        self.control.sockets.lock().unwrap_or_else(|error| error.into_inner()).remove(&self.key);
        self.control.settled.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tickets_are_single_use_and_bound_to_control() {
        let control = DesktopControl::default();
        assert!(control.mint("a", 0).await.is_err());
        control.activate("a", 100).await.unwrap();
        assert!(control.mint("b", 0).await.is_err());
        let ticket = control.mint("a", 0).await.unwrap();
        assert_eq!(control.consume(&ticket).as_deref(), Some("a"));
        assert!(control.consume(&ticket).is_none());
        assert!(control.mint("a", 100).await.is_err());
    }

    #[tokio::test]
    async fn revocation_waits_for_live_sockets_and_stale_return_cannot_revoke_new_owner() {
        let control = Arc::new(DesktopControl::default());
        control.activate("a", 100).await.unwrap();
        let ticket = control.mint("a", 0).await.unwrap();
        let (guard, mut cancellation) = control.register("a", 0).await.unwrap();
        let pending = tokio::spawn({ let control = control.clone(); async move { control.revoke("a").await } });
        cancellation.changed().await.unwrap();
        assert!(*cancellation.borrow());
        assert!(!pending.is_finished());
        drop(guard);
        pending.await.unwrap().unwrap();
        assert!(control.consume(&ticket).is_none());
        control.activate("b", 200).await.unwrap();
        control.revoke("a").await.unwrap();
        assert!(control.mint("b", 100).await.is_ok());
    }
}
