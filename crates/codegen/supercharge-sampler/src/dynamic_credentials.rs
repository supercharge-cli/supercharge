//! Process-wide credential sources consulted on every request.
//!
//! Some providers issue short-lived tokens that are refreshed out of band (for example an
//! OAuth login stored on disk). A [`DynamicCredentialSource`] is asked for the bearer right
//! before each request, so a token saved after the session started is picked up without a
//! restart. Sources only apply to clients without their own [`crate::config::BearerResolver`].

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

/// A source of fresh bearer tokens for particular provider base URLs.
pub trait DynamicCredentialSource: Send + Sync + std::fmt::Debug {
    /// Bearer to send to `base_url`, or `None` when this source does not cover it.
    fn bearer_for(&self, base_url: &str) -> Option<String>;

    /// Awaited before a request to `base_url`; refresh an expiring token here.
    fn prepare<'a>(&'a self, _base_url: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}

fn sources() -> &'static RwLock<Vec<Arc<dyn DynamicCredentialSource>>> {
    static SOURCES: std::sync::OnceLock<RwLock<Vec<Arc<dyn DynamicCredentialSource>>>> =
        std::sync::OnceLock::new();
    SOURCES.get_or_init(|| RwLock::new(Vec::new()))
}

/// Register a source. Registering the same source type twice is the caller's concern.
pub fn register(source: Arc<dyn DynamicCredentialSource>) {
    sources().write().unwrap_or_else(|e| e.into_inner()).push(source);
}

fn snapshot() -> Vec<Arc<dyn DynamicCredentialSource>> {
    sources().read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The first registered source's bearer for `base_url`.
pub fn bearer_for(base_url: &str) -> Option<String> {
    snapshot().iter().find_map(|s| s.bearer_for(base_url))
}

/// Let every source refresh before a request to `base_url`.
pub async fn prepare(base_url: &str) {
    for source in snapshot() {
        source.prepare(base_url).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Fixed;
    impl DynamicCredentialSource for Fixed {
        fn bearer_for(&self, base_url: &str) -> Option<String> {
            base_url.contains("dyn-cred.test").then(|| "tok-1".to_owned())
        }
    }

    #[test]
    fn registered_source_answers_only_its_hosts() {
        register(Arc::new(Fixed));
        assert_eq!(bearer_for("https://dyn-cred.test/v1").as_deref(), Some("tok-1"));
        assert_eq!(bearer_for("https://other.test/v1"), None);
    }
}
