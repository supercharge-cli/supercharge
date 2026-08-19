//! Logging interface for SDK diagnostics.

/// Logging sink for SDK diagnostics, mirroring the JS `Logger` interface.
///
/// All methods default to no-ops, so implementors override only the levels
/// they care about. Pass an `Arc<dyn Logger>` through the connection options.
///
/// # Examples
///
/// ```
/// use e2b_rs::Logger;
/// use std::sync::Mutex;
///
/// #[derive(Default)]
/// struct Collect(Mutex<Vec<String>>);
/// impl Logger for Collect {
///     fn info(&self, message: &str) {
///         if let Ok(mut v) = self.0.lock() {
///             v.push(message.to_string());
///         }
///     }
/// }
///
/// let logger = Collect::default();
/// logger.info("sandbox created");
/// ```
pub trait Logger: Send + Sync {
    /// Log a debug-level message.
    fn debug(&self, message: &str) {
        let _ = message;
    }
    /// Log an info-level message.
    fn info(&self, message: &str) {
        let _ = message;
    }
    /// Log a warning-level message.
    fn warn(&self, message: &str) {
        let _ = message;
    }
    /// Log an error-level message.
    fn error(&self, message: &str) {
        let _ = message;
    }
}

/// A [`Logger`] that discards every message.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopLogger;

impl Logger for NoopLogger {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Capture {
        msgs: Mutex<Vec<String>>,
    }
    impl Logger for Capture {
        fn info(&self, message: &str) {
            self.msgs.lock().unwrap().push(format!("info:{message}"));
        }
        fn error(&self, message: &str) {
            self.msgs.lock().unwrap().push(format!("error:{message}"));
        }
    }

    #[test]
    fn only_overridden_levels_record() {
        let c = Capture::default();
        c.debug("ignored"); // default no-op
        c.warn("ignored"); // default no-op
        c.info("hello");
        c.error("boom");
        let msgs = c.msgs.lock().unwrap();
        assert_eq!(
            &*msgs,
            &["info:hello".to_string(), "error:boom".to_string()]
        );
    }

    #[test]
    fn noop_logger_is_silent() {
        let logger = NoopLogger;
        logger.debug("x");
        logger.info("x");
        logger.warn("x");
        logger.error("x");
        // No panic, nothing recorded — just verifies it compiles and runs.
    }

    #[test]
    fn logger_is_object_safe() {
        let logger: std::sync::Arc<dyn Logger> = std::sync::Arc::new(NoopLogger);
        logger.info("works behind a trait object");
    }
}
