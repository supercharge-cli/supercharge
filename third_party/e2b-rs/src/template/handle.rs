//! Build handle — live streaming access to a running template build.
//!
//! [`BuildHandle`] wraps a background poll task and an [`mpsc`] log channel,
//! mirroring the design of `CommandHandle` from the sandbox module.
//!
//! The internal `wait_for_build_finish` function drives the poll loop: it
//! calls the build-status endpoint in a loop, forwarding [`LogEntry`] items
//! until the build reaches a terminal state ([`BuildStatus::Ready`] or
//! [`BuildStatus::Error`]).
//!
//! # Re-exports
//!
//! [`BuildHandle`] is re-exported at the crate root.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::api::client::ApiClient;
use crate::errors::{Error, Result};
use crate::template::build_api::get_build_status;
use crate::template::log::LogEntry;
use crate::template::types::{BuildInfo, BuildStatus};

// ── wait_for_build_finish ────────────────────────────────────────────────────

/// Drive the build-status poll loop to completion.
///
/// Polls `GET /templates/{template_id}/builds/{build_id}/status` in a loop,
/// forwarding each new [`LogEntry`] over `logs`. When the build reaches
/// [`BuildStatus::Ready`] the function returns `Ok(())`. When the build
/// reaches [`BuildStatus::Error`] it returns
/// [`Error::Build`][crate::errors::Error::Build] with the API-supplied reason
/// message.
///
/// # Arguments
///
/// - `api` — shared API client kept alive for the duration of the task.
/// - `template_id` / `build_id` — identifiers of the build to poll.
/// - `logs_refresh_frequency_ms` — milliseconds to sleep between polls while
///   the build is still in progress.
/// - `logs` — sender half of an [`mpsc`][tokio::sync::mpsc] channel; send
///   errors are silently ignored in case the receiver has been dropped.
///
/// # Errors
///
/// Returns an error if any HTTP poll call fails, or
/// [`Error::Build`][crate::errors::Error::Build] when the build status is
/// [`BuildStatus::Error`].
pub(crate) async fn wait_for_build_finish(
    api: Arc<ApiClient>,
    template_id: String,
    build_id: String,
    logs_refresh_frequency_ms: u64,
    logs: mpsc::Sender<LogEntry>,
) -> Result<()> {
    let mut logs_offset: usize = 0;
    let mut status = BuildStatus::Building;

    while status == BuildStatus::Building || status == BuildStatus::Waiting {
        let resp = get_build_status(&api, &template_id, &build_id, logs_offset).await?;
        logs_offset += resp.log_entries.len();
        for entry in &resp.log_entries {
            let _ = logs.send(entry.clone()).await;
        }
        status = resp.status;

        match status {
            BuildStatus::Ready | BuildStatus::Error => {
                // Capture the error message before the drain loop so we can
                // return it after all remaining log entries have been flushed.
                let error_msg = if status == BuildStatus::Error {
                    resp.reason
                        .as_ref()
                        .map(|r| r.message.clone())
                        .unwrap_or_else(|| "Unknown error".to_string())
                } else {
                    String::new()
                };

                // Drain any trailing log entries the API may have buffered.
                // The terminal response already had its entries forwarded above;
                // check whether it was non-empty, and if so keep fetching until
                // we receive an empty batch (mirroring the JS `waitForBuildFinish`
                // drain loop in `buildApi.ts`).
                let mut tail = resp;
                while !tail.log_entries.is_empty() {
                    tail = get_build_status(&api, &template_id, &build_id, logs_offset).await?;
                    logs_offset += tail.log_entries.len();
                    for entry in &tail.log_entries {
                        let _ = logs.send(entry.clone()).await;
                    }
                }

                return if status == BuildStatus::Ready {
                    Ok(())
                } else {
                    Err(Error::Build(error_msg))
                };
            }
            BuildStatus::Building | BuildStatus::Waiting => {
                tokio::time::sleep(Duration::from_millis(logs_refresh_frequency_ms)).await;
            }
        }
    }

    // Unreachable in normal flow — the loop always returns through the
    // Ready/Error match arm.  Mirrors the JS SDK's post-loop
    // `throw new BuildError('Unknown build error occurred.')`.
    Err(Error::Build("Unknown build error occurred.".to_string()))
}

// ── BuildHandle ──────────────────────────────────────────────────────────────

/// A handle for a running template build.
///
/// Returned by [`crate::template::Template::build`]. Exposes two ways to
/// interact with the build:
///
/// - [`BuildHandle::next`] — pull the next [`LogEntry`] from the live log
///   stream one at a time.
/// - [`BuildHandle::wait`] — drain all remaining log entries and block until
///   the build completes, returning [`BuildInfo`] on success or an error on
///   failure.
///
/// # Cancellation
///
/// Dropping a [`BuildHandle`] aborts the background poll task. The build
/// continues on E2B infrastructure; only local log streaming is terminated.
pub struct BuildHandle {
    logs: mpsc::Receiver<LogEntry>,
    result: Option<oneshot::Receiver<Result<BuildInfo>>>,
    task: JoinHandle<()>,
    info: BuildInfo,
}

impl BuildHandle {
    /// Construct a [`BuildHandle`] from its constituent channels and task.
    ///
    /// Called by the build orchestration layer in
    /// [`crate::template::Template::build`] after spawning the poll task.
    pub(crate) fn new(
        logs: mpsc::Receiver<LogEntry>,
        result: oneshot::Receiver<Result<BuildInfo>>,
        task: JoinHandle<()>,
        info: BuildInfo,
    ) -> Self {
        Self {
            logs,
            result: Some(result),
            task,
            info,
        }
    }

    /// The template identifier for the running build.
    pub fn template_id(&self) -> &str {
        &self.info.template_id
    }

    /// The build identifier for the running build.
    pub fn build_id(&self) -> &str {
        &self.info.build_id
    }

    /// Return the next [`LogEntry`] from the build log stream.
    ///
    /// Returns `None` when the stream is closed, i.e. the build has completed
    /// (successfully or with an error) and all log entries have been consumed.
    pub async fn next(&mut self) -> Option<LogEntry> {
        self.logs.recv().await
    }

    /// Drain all remaining log entries and wait for the build to complete.
    ///
    /// Consumes `self`. After this call returns there are no remaining log
    /// entries to read.
    ///
    /// # Returns
    ///
    /// - `Ok(BuildInfo)` — the build completed successfully.
    /// - `Err(Error::Build(_))` — the build failed; the message contains the
    ///   API-supplied reason.
    /// - `Err(Error::Internal(_))` — `wait` was called more than once, or the
    ///   background task ended without sending a result.
    pub async fn wait(mut self) -> Result<BuildInfo> {
        // Drain any log entries the caller has not consumed.
        while self.logs.recv().await.is_some() {}
        let rx = self
            .result
            .take()
            .ok_or_else(|| Error::Internal("wait called twice".to_string()))?;
        match rx.await {
            Ok(outcome) => outcome,
            Err(_) => Err(Error::Internal("build ended without a result".to_string())),
        }
    }
}

impl Drop for BuildHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection_config::{ConnectionConfig, ConnectionConfigOpts};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn api_client_for(server: &MockServer) -> Arc<ApiClient> {
        let config = ConnectionConfig::new(ConnectionConfigOpts {
            api_key: Some("e2b_0123456789abcdef".to_string()),
            api_url: Some(server.uri()),
            ..Default::default()
        });
        Arc::new(ApiClient::new(&config, true).expect("api client"))
    }

    fn make_build_info() -> BuildInfo {
        BuildInfo {
            template_id: "tpl_1".to_string(),
            build_id: "bld_1".to_string(),
            name: None,
            alias: None,
            tags: vec![],
        }
    }

    /// `wait_for_build_finish` should stream log entries from a `"building"`
    /// response and complete successfully when the build transitions to
    /// `"ready"`.
    ///
    /// Mock ordering: wiremock 0.6 matches mocks in *registration order*
    /// (first registered = first tried). We therefore register `"building"`
    /// first (fires once via `up_to_n_times(1)`) and `"ready"` second
    /// (acts as the fallback once `"building"` is exhausted).
    #[tokio::test]
    async fn build_streams_logs_then_ready() {
        let server = MockServer::start().await;

        // First — responds "building" + one log entry exactly once.
        Mock::given(method("GET"))
            .and(path("/templates/tpl_1/builds/bld_1/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "building",
                "logEntries": [
                    {
                        "level": "info",
                        "message": "step 1",
                        "timestamp": "2024-01-01T00:00:00Z"
                    }
                ],
                "logs": [],
                "templateID": "tpl_1",
                "buildID": "bld_1"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        // Fallback — responds "ready" once "building" is exhausted.
        Mock::given(method("GET"))
            .and(path("/templates/tpl_1/builds/bld_1/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "ready",
                "logEntries": [],
                "logs": [],
                "templateID": "tpl_1",
                "buildID": "bld_1"
            })))
            .mount(&server)
            .await;

        let api = api_client_for(&server);
        let info = make_build_info();

        let (tx_logs, rx_logs) = mpsc::channel::<LogEntry>(16);
        let (tx_result, rx_result) = oneshot::channel::<Result<BuildInfo>>();

        let api_arc = Arc::clone(&api);
        let info_clone = info.clone();
        let task = tokio::spawn(async move {
            let r = wait_for_build_finish(
                api_arc,
                "tpl_1".to_string(),
                "bld_1".to_string(),
                10,
                tx_logs,
            )
            .await
            .map(|()| info_clone);
            let _ = tx_result.send(r);
        });

        let mut handle = BuildHandle::new(rx_logs, rx_result, task, info);

        // First log entry from the "building" response.
        let entry = handle.next().await.expect("expected a log entry");
        assert_eq!(entry.message(), "step 1");

        // Wait for completion — drains any remaining entries then reads the result.
        let result = handle.wait().await.expect("build should succeed");
        assert_eq!(result.template_id, "tpl_1");
    }

    /// When the build status endpoint returns `"error"`, [`BuildHandle::wait`]
    /// must return [`Error::Build`] containing the API-supplied reason message.
    #[tokio::test]
    async fn build_error_status_fails() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/templates/tpl_1/builds/bld_1/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "error",
                "reason": {
                    "message": "boom",
                    "logEntries": []
                },
                "logEntries": [],
                "logs": [],
                "templateID": "tpl_1",
                "buildID": "bld_1"
            })))
            .mount(&server)
            .await;

        let api = api_client_for(&server);
        let info = make_build_info();

        let (tx_logs, rx_logs) = mpsc::channel::<LogEntry>(16);
        let (tx_result, rx_result) = oneshot::channel::<Result<BuildInfo>>();

        let api_arc = Arc::clone(&api);
        let info_clone = info.clone();
        let task = tokio::spawn(async move {
            let r = wait_for_build_finish(
                api_arc,
                "tpl_1".to_string(),
                "bld_1".to_string(),
                10,
                tx_logs,
            )
            .await
            .map(|()| info_clone);
            let _ = tx_result.send(r);
        });

        let handle = BuildHandle::new(rx_logs, rx_result, task, info);
        let err = handle.wait().await.expect_err("build should fail");
        match err {
            Error::Build(msg) => assert!(msg.contains("boom"), "expected 'boom' in: {msg}"),
            other => panic!("expected Error::Build, got {other:?}"),
        }
    }

    /// [`BuildHandle::template_id`] and [`BuildHandle::build_id`] return the
    /// identifiers without awaiting or consuming the handle.
    #[tokio::test]
    async fn accessors_return_ids() {
        let (_tx_logs, rx_logs) = mpsc::channel::<LogEntry>(1);
        let (_tx_result, rx_result) = oneshot::channel::<Result<BuildInfo>>();
        let task = tokio::spawn(async {});
        let info = make_build_info();
        let handle = BuildHandle::new(rx_logs, rx_result, task, info);
        assert_eq!(handle.template_id(), "tpl_1");
        assert_eq!(handle.build_id(), "bld_1");
    }
}
