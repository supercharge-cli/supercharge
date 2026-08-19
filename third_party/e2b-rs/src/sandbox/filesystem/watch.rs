//! Directory watching (`watch_dir`) over the Connect `WatchDir` server stream.

use futures::StreamExt as _;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::{Filesystem, FilesystemEvent, file_not_found_on_missing};
use crate::envd::proto::filesystem as pb;
use crate::envd::proto::filesystem::watch_dir_response::Event as WatchEvent;
use crate::envd::versions::{
    ENVD_VERSION_FS_EVENT_ENTRY_INFO, ENVD_VERSION_RECURSIVE_WATCH,
    ENVD_VERSION_WATCH_NETWORK_MOUNTS, version_gte,
};
use crate::errors::{Error, Result};

/// Options for [`Filesystem::watch_dir`].
#[derive(Default)]
pub struct WatchOpts {
    /// Watch subdirectories recursively (requires envd >= 0.1.4).
    pub recursive: bool,
    /// Include the affected entry's info in events (requires envd >= 0.6.3).
    pub include_entry: bool,
    /// Allow watching network-mounted paths (requires envd >= 0.6.4).
    pub allow_network_mounts: bool,
    /// The sandbox user.
    pub user: Option<String>,
}

/// A live directory watch. Receive events with [`WatchHandle::next`]; the watch
/// stops when the handle is dropped or [`WatchHandle::stop`] is called, or when
/// the server closes the stream.
pub struct WatchHandle {
    events: mpsc::Receiver<FilesystemEvent>,
    task: JoinHandle<()>,
}

impl WatchHandle {
    /// Receive the next filesystem event, or `None` when the watch has ended
    /// (server closed the stream, errored, or the watch was stopped).
    pub async fn next(&mut self) -> Option<FilesystemEvent> {
        self.events.recv().await
    }

    /// Borrow the underlying receiver (e.g. to use in `tokio::select!`).
    pub fn events(&mut self) -> &mut mpsc::Receiver<FilesystemEvent> {
        &mut self.events
    }

    /// Stop watching (aborts the background task + underlying request).
    pub fn stop(self) {
        self.task.abort();
        // `self` (incl. the receiver + JoinHandle) drops here; Drop also aborts.
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Filesystem {
    /// Watch a directory for changes. Returns a [`WatchHandle`] whose
    /// [`WatchHandle::next`] yields [`FilesystemEvent`]s. The stream's initial
    /// `StartEvent` handshake is consumed internally; `KeepAlive` frames are
    /// dropped.
    ///
    /// # Errors
    /// Returns [`Error::Template`] if a requested option (recursive /
    /// include_entry / allow_network_mounts) is unsupported by the sandbox's envd.
    pub async fn watch_dir(&self, path: &str, opts: WatchOpts) -> Result<WatchHandle> {
        if opts.recursive && !version_gte(&self.envd_version, ENVD_VERSION_RECURSIVE_WATCH) {
            return Err(Error::Template(format!(
                "Recursive watch requires envd >= {ENVD_VERSION_RECURSIVE_WATCH}"
            )));
        }
        if opts.include_entry && !version_gte(&self.envd_version, ENVD_VERSION_FS_EVENT_ENTRY_INFO)
        {
            return Err(Error::Template(format!(
                "include_entry requires envd >= {ENVD_VERSION_FS_EVENT_ENTRY_INFO}"
            )));
        }
        if opts.allow_network_mounts
            && !version_gte(&self.envd_version, ENVD_VERSION_WATCH_NETWORK_MOUNTS)
        {
            return Err(Error::Template(format!(
                "allow_network_mounts requires envd >= {ENVD_VERSION_WATCH_NETWORK_MOUNTS}"
            )));
        }

        let user = self.resolve_user(opts.user.as_deref());
        let req = pb::WatchDirRequest {
            path: path.to_string(),
            recursive: opts.recursive,
            include_entry: opts.include_entry,
            allow_network_mounts: opts.allow_network_mounts,
        };
        let stream = self
            .connect
            .server_stream::<_, pb::WatchDirResponse>(
                crate::connect::FS_WATCH_DIR,
                &req,
                user.as_deref(),
            )
            .await
            .map_err(|e| file_not_found_on_missing(e, path))?;

        let (tx, rx) = mpsc::channel(64);
        let task = tokio::spawn(async move {
            futures::pin_mut!(stream);
            while let Some(item) = stream.next().await {
                let Ok(resp) = item else { break }; // stream error ends the watch
                // StartEvent + KeepAlive are not surfaced to the consumer.
                if let Some(WatchEvent::Filesystem(ev)) = resp.event
                    && let Some(mapped) = FilesystemEvent::from_proto(ev)
                    && tx.send(mapped).await.is_err()
                {
                    break; // receiver dropped
                }
            }
        });

        Ok(WatchHandle { events: rx, task })
    }
}

#[cfg(test)]
mod tests {
    use crate::connect::envelope::{FLAG_END_STREAM, encode_envelope};
    use crate::connection_config::{ConnectionConfig, ConnectionConfigOpts};
    use crate::sandbox::filesystem::{Filesystem, FilesystemEventType, WatchOpts};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fs_for(server: &MockServer) -> Filesystem {
        let config = ConnectionConfig::new(ConnectionConfigOpts::default());
        Filesystem::build_with_base_url(server.uri(), "sbx_w", "0.6.4", None, &config)
            .expect("filesystem")
    }

    #[tokio::test]
    async fn watch_yields_mapped_events_then_ends() {
        let server = MockServer::start().await;
        // StartEvent, one FilesystemEvent, a KeepAlive (must be dropped), end-stream.
        let mut body = encode_envelope(0, br#"{"start":{}}"#);
        body.extend(encode_envelope(
            0,
            br#"{"filesystem":{"name":"a.txt","type":"EVENT_TYPE_CREATE"}}"#,
        ));
        body.extend(encode_envelope(0, br#"{"keepalive":{}}"#));
        body.extend(encode_envelope(FLAG_END_STREAM, b"{}"));
        Mock::given(method("POST"))
            .and(path("/filesystem.Filesystem/WatchDir"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+json")
                    .set_body_bytes(body),
            )
            .mount(&server)
            .await;

        let fs = fs_for(&server);
        let mut handle = fs
            .watch_dir("/d", WatchOpts::default())
            .await
            .expect("watch");
        let ev = handle.next().await.expect("event");
        assert_eq!(ev.r#type, FilesystemEventType::Create);
        assert_eq!(ev.name, "a.txt");
        // StartEvent + KeepAlive were not surfaced; stream ended after the event.
        assert!(handle.next().await.is_none());
    }
}
