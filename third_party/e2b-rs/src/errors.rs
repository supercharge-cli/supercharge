//! Error types for the E2B SDK.
//!
//! Mirrors the error hierarchy of the JavaScript SDK (`src/errors.ts`). Rust
//! has no class inheritance, so the JS subclass relationships are modeled as
//! sibling variants plus the [`Error::is_not_found`], [`Error::is_authentication`],
//! and [`Error::is_build`] predicates.

/// Convenient result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// All errors returned by the E2B SDK.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// General sandbox error (JS `SandboxError`).
    #[error("{0}")]
    Sandbox(String),
    /// A timeout, often caused by the sandbox itself timing out (JS `TimeoutError`).
    #[error("{0}")]
    Timeout(String),
    /// An invalid argument was supplied (JS `InvalidArgumentError`).
    #[error("{0}")]
    InvalidArgument(String),
    /// The sandbox ran out of disk space (JS `NotEnoughSpaceError`).
    #[error("{0}")]
    NotEnoughSpace(String),
    /// A resource was not found (JS deprecated `NotFoundError`).
    #[error("{0}")]
    NotFound(String),
    /// A file or directory was not found inside the sandbox (JS `FileNotFoundError`).
    #[error("{0}")]
    FileNotFound(String),
    /// The sandbox was not found or is no longer running (JS `SandboxNotFoundError`).
    #[error("{0}")]
    SandboxNotFound(String),
    /// Authentication failed (JS `AuthenticationError`).
    #[error("{0}")]
    Authentication(String),
    /// The request was authenticated but not authorized (HTTP 403). For some
    /// existence probes (e.g. a template alias owned by another team) this is a
    /// meaningful "exists but not yours" signal rather than a hard failure.
    #[error("{0}")]
    Forbidden(String),
    /// Git authentication failed (JS `GitAuthError`).
    #[error("{0}")]
    GitAuth(String),
    /// Git upstream tracking is missing (JS `GitUpstreamError`).
    #[error("{0}")]
    GitUpstream(String),
    /// The template uses an incompatible envd version (JS `TemplateError`).
    #[error("{0}")]
    Template(String),
    /// The API rate limit was exceeded (JS `RateLimitError`).
    #[error("{0}")]
    RateLimit(String),
    /// The request conflicts with the resource's current state (HTTP 409),
    /// e.g. pausing an already-paused sandbox or updating a paused sandbox.
    #[error("{0}")]
    Conflict(String),
    /// A template build failed (JS `BuildError`).
    #[error("{0}")]
    Build(String),
    /// A build file upload failed (JS `FileUploadError`).
    #[error("{0}")]
    FileUpload(String),
    /// A volume operation failed (JS `VolumeError`).
    #[error("{0}")]
    Volume(String),
    /// A command exited with a non-zero status (JS `CommandExitError`).
    #[error("command exited with code {exit_code}")]
    CommandExit {
        /// Process exit code.
        exit_code: i32,
        /// Accumulated stdout.
        stdout: String,
        /// Accumulated stderr.
        stderr: String,
        /// Optional error string reported by envd.
        error: Option<String>,
    },
    /// Underlying HTTP transport error (connection, TLS, timeout at the wire level).
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
    /// Internal invariant violation. Used instead of panicking in "impossible"
    /// cases so the library never aborts the host process.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Build the sandbox-timeout error message used for 502/Unavailable responses,
/// matching `formatSandboxTimeoutError` in the JS SDK.
pub fn format_sandbox_timeout_error(message: impl Into<String>) -> Error {
    let message = message.into();
    Error::Timeout(format!(
        "{message}: This error is likely due to sandbox timeout. You can modify the \
         sandbox timeout by passing 'timeoutMs' when starting the sandbox or calling \
         '.setTimeout' on the sandbox with the desired timeout."
    ))
}

impl Error {
    /// Map an HTTP status code to a typed error. Mirrors the envd/control-plane
    /// default error maps in the JS SDK (the comprehensive superset).
    pub fn from_status(status: u16, message: impl Into<String>) -> Error {
        let message = message.into();
        match status {
            400 => Error::InvalidArgument(message),
            401 => Error::Authentication(message),
            403 => Error::Forbidden(message),
            404 => Error::NotFound(message),
            409 => Error::Conflict(message),
            429 => Error::RateLimit(message),
            502 => format_sandbox_timeout_error(message),
            507 => Error::NotEnoughSpace(message),
            _ => Error::Sandbox(message),
        }
    }

    /// True for `NotFound` and its JS subtypes (`FileNotFound`, `SandboxNotFound`).
    pub fn is_not_found(&self) -> bool {
        matches!(
            self,
            Error::NotFound(_) | Error::FileNotFound(_) | Error::SandboxNotFound(_)
        )
    }

    /// True for `Authentication` and its JS subtype (`GitAuth`).
    pub fn is_authentication(&self) -> bool {
        matches!(self, Error::Authentication(_) | Error::GitAuth(_))
    }

    /// True for `Build` and its JS subtype (`FileUpload`).
    pub fn is_build(&self) -> bool {
        matches!(self, Error::Build(_) | Error::FileUpload(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_status_maps_known_codes() {
        assert!(matches!(
            Error::from_status(400, "x"),
            Error::InvalidArgument(_)
        ));
        assert!(matches!(
            Error::from_status(401, "x"),
            Error::Authentication(_)
        ));
        assert!(matches!(Error::from_status(404, "x"), Error::NotFound(_)));
        assert!(matches!(Error::from_status(409, "x"), Error::Conflict(_)));
        assert!(matches!(Error::from_status(429, "x"), Error::RateLimit(_)));
        assert!(matches!(
            Error::from_status(507, "x"),
            Error::NotEnoughSpace(_)
        ));
        assert!(matches!(Error::from_status(500, "x"), Error::Sandbox(_)));
    }

    #[test]
    fn from_status_502_is_timeout_with_hint() {
        let err = Error::from_status(502, "boom");
        match err {
            Error::Timeout(msg) => {
                assert!(msg.contains("boom"));
                assert!(msg.contains("sandbox timeout"));
            }
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[test]
    fn not_found_predicate_groups_subtypes() {
        assert!(Error::NotFound("a".into()).is_not_found());
        assert!(Error::FileNotFound("a".into()).is_not_found());
        assert!(Error::SandboxNotFound("a".into()).is_not_found());
        assert!(!Error::Sandbox("a".into()).is_not_found());
    }

    #[test]
    fn auth_and_build_predicates_group_subtypes() {
        assert!(Error::Authentication("a".into()).is_authentication());
        assert!(Error::GitAuth("a".into()).is_authentication());
        assert!(!Error::Sandbox("a".into()).is_authentication());

        assert!(Error::Build("a".into()).is_build());
        assert!(Error::FileUpload("a".into()).is_build());
        assert!(!Error::Sandbox("a".into()).is_build());
    }

    #[tokio::test]
    async fn reqwest_error_converts_via_from() {
        // A connection to an unroutable address yields a reqwest::Error,
        // which must convert into Error::Transport via `?`/`#[from]`.
        fn try_it(e: reqwest::Error) -> Error {
            Error::from(e)
        }
        let err = reqwest::get("http://127.0.0.1:1/nope").await.unwrap_err();
        assert!(matches!(try_it(err), Error::Transport(_)));
    }

    #[test]
    fn display_renders_message_and_command_exit() {
        assert_eq!(Error::Sandbox("boom".into()).to_string(), "boom");
        let ce = Error::CommandExit {
            exit_code: 2,
            stdout: "out".into(),
            stderr: "err".into(),
            error: Some("bad".into()),
        };
        assert!(ce.to_string().contains("exited with code 2"));
    }
}
