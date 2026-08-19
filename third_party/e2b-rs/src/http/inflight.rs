//! FIFO concurrency limiter for outbound requests (port of `api/inflight.ts`).

use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// A FIFO cap on concurrent in-flight requests. `max == 0` disables the cap
/// (every `acquire` returns `None`, matching JS `limitConcurrency(max<=0)`).
#[derive(Clone)]
pub(crate) struct ConcurrencyLimiter {
    sem: Option<Arc<Semaphore>>,
}

#[allow(dead_code)] // used by ApiClient in Task 3
impl ConcurrencyLimiter {
    /// Create a limiter allowing `max` concurrent holders (`0` = unlimited).
    pub(crate) fn new(max: usize) -> Self {
        let sem = if max == 0 {
            None
        } else {
            Some(Arc::new(Semaphore::new(max)))
        };
        Self { sem }
    }

    /// Acquire a slot, waiting (FIFO) if the cap is reached. Returns `None` when
    /// the limiter is disabled. Hold the returned permit for the request's
    /// lifetime; dropping it frees the slot.
    pub(crate) async fn acquire(&self) -> Option<OwnedSemaphorePermit> {
        match &self.sem {
            None => None,
            // Semaphore is never closed in our usage; treat closure as "no cap".
            Some(sem) => sem.clone().acquire_owned().await.ok(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn caps_concurrent_holders() {
        let limiter = Arc::new(ConcurrencyLimiter::new(2));
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let (l, live, peak) = (limiter.clone(), live.clone(), peak.clone());
            handles.push(tokio::spawn(async move {
                let _permit = l.acquire().await;
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                live.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.expect("task");
        }
        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "never more than 2 in flight"
        );
    }

    #[tokio::test]
    async fn disabled_limiter_returns_none_permit() {
        let limiter = ConcurrencyLimiter::new(0);
        assert!(limiter.acquire().await.is_none());
    }
}
