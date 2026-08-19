//! Connect protocol error codes and mapping to the SDK error type.

use crate::errors::{Error, format_sandbox_timeout_error};

/// Connect protocol status code (mirrors `e2b_connect.Code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Code {
    // used by Task 4
    #[allow(dead_code)]
    Canceled,
    // used by Task 4
    #[allow(dead_code)]
    Unknown,
    // used by Task 4
    #[allow(dead_code)]
    InvalidArgument,
    // used by Task 4
    #[allow(dead_code)]
    DeadlineExceeded,
    // used by Task 4
    #[allow(dead_code)]
    NotFound,
    // used by Task 4
    #[allow(dead_code)]
    AlreadyExists,
    // used by Task 4
    #[allow(dead_code)]
    PermissionDenied,
    // used by Task 4
    #[allow(dead_code)]
    ResourceExhausted,
    // used by Task 4
    #[allow(dead_code)]
    FailedPrecondition,
    // used by Task 4
    #[allow(dead_code)]
    Aborted,
    // used by Task 4
    #[allow(dead_code)]
    OutOfRange,
    // used by Task 4
    #[allow(dead_code)]
    Unimplemented,
    // used by Task 4
    #[allow(dead_code)]
    Internal,
    // used by Task 4
    #[allow(dead_code)]
    Unavailable,
    // used by Task 4
    #[allow(dead_code)]
    DataLoss,
    // used by Task 4
    #[allow(dead_code)]
    Unauthenticated,
}

impl Code {
    /// Parse a Connect code name (e.g. `"not_found"`); unknown names → [`Code::Unknown`].
    // used by Task 4
    #[allow(dead_code)]
    pub(crate) fn from_name(name: &str) -> Code {
        match name {
            "canceled" => Code::Canceled,
            "invalid_argument" => Code::InvalidArgument,
            "deadline_exceeded" => Code::DeadlineExceeded,
            "not_found" => Code::NotFound,
            "already_exists" => Code::AlreadyExists,
            "permission_denied" => Code::PermissionDenied,
            "resource_exhausted" => Code::ResourceExhausted,
            "failed_precondition" => Code::FailedPrecondition,
            "aborted" => Code::Aborted,
            "out_of_range" => Code::OutOfRange,
            "unimplemented" => Code::Unimplemented,
            "internal" => Code::Internal,
            "unavailable" => Code::Unavailable,
            "data_loss" => Code::DataLoss,
            "unauthenticated" => Code::Unauthenticated,
            _ => Code::Unknown,
        }
    }

    /// Map an HTTP status to a Connect code (mirrors `make_error_from_http_code`).
    // used by Task 4
    #[allow(dead_code)]
    pub(crate) fn from_http_status(status: u16) -> Code {
        match status {
            400 => Code::InvalidArgument,
            401 => Code::Unauthenticated,
            403 => Code::PermissionDenied,
            404 => Code::NotFound,
            409 => Code::AlreadyExists,
            413 | 429 => Code::ResourceExhausted,
            499 => Code::Canceled,
            500 => Code::Internal,
            501 | 505 => Code::Unimplemented,
            502 | 503 => Code::Unavailable,
            504 => Code::DeadlineExceeded,
            _ => Code::Unknown,
        }
    }
}

/// Parse a Connect error from a response/end-stream body. A JSON `{code, message}`
/// where `code` is an integer uses the HTTP table; a string `code` is the name;
/// a non-JSON body falls back to `(from_http_status(status), <body text>)`.
// used by Task 4
#[allow(dead_code)]
pub(crate) fn parse_connect_error(status: u16, body: &[u8]) -> (Code, String) {
    #[derive(serde::Deserialize)]
    struct Raw {
        code: Option<serde_json::Value>,
        #[serde(default)]
        message: String,
    }
    match serde_json::from_slice::<Raw>(body) {
        Ok(raw) => {
            let code = match raw.code {
                Some(serde_json::Value::String(s)) => Code::from_name(&s),
                Some(serde_json::Value::Number(n)) => {
                    Code::from_http_status(u16::try_from(n.as_u64().unwrap_or(0)).unwrap_or(0))
                }
                _ => Code::from_http_status(status),
            };
            (code, raw.message)
        }
        Err(_) => (
            Code::from_http_status(status),
            String::from_utf8_lossy(body).into_owned(),
        ),
    }
}

/// Map a Connect [`Code`] + message to the SDK [`Error`]. Mirrors `rpc.ts`'s
/// `DEFAULT_ERROR_MAP`; codes not in that map become a generic [`Error::Sandbox`].
// used by Task 4
#[allow(dead_code)]
pub(crate) fn map_code_to_error(code: Code, message: String) -> Error {
    match code {
        Code::InvalidArgument => Error::InvalidArgument(message),
        Code::Unauthenticated => Error::Authentication(message),
        Code::NotFound => Error::NotFound(message),
        Code::ResourceExhausted => Error::RateLimit(message),
        Code::Unavailable => format_sandbox_timeout_error(message),
        Code::Canceled | Code::DeadlineExceeded => Error::Timeout(message),
        Code::AlreadyExists => Error::Conflict(message),
        _ => Error::Sandbox(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::Error;

    #[test]
    fn code_from_name_and_http() {
        assert_eq!(Code::from_name("not_found"), Code::NotFound);
        assert_eq!(
            Code::from_name("resource_exhausted"),
            Code::ResourceExhausted
        );
        assert_eq!(Code::from_name("totally_unknown"), Code::Unknown);
        assert_eq!(Code::from_http_status(404), Code::NotFound);
        assert_eq!(Code::from_http_status(429), Code::ResourceExhausted);
        assert_eq!(Code::from_http_status(502), Code::Unavailable);
        assert_eq!(Code::from_http_status(418), Code::Unknown);
    }

    #[test]
    fn parse_connect_error_string_int_and_nonjson() {
        // String code → used directly.
        let (c, m) = parse_connect_error(500, br#"{"code":"not_found","message":"nope"}"#);
        assert_eq!(c, Code::NotFound);
        assert_eq!(m, "nope");
        // Integer code → mapped via HTTP table.
        let (c, m) = parse_connect_error(200, br#"{"code":429,"message":"slow down"}"#);
        assert_eq!(c, Code::ResourceExhausted);
        assert_eq!(m, "slow down");
        // Non-JSON body → fall back to (from_http_status, body text).
        let (c, m) = parse_connect_error(404, b"plain text error");
        assert_eq!(c, Code::NotFound);
        assert_eq!(m, "plain text error");
    }

    #[test]
    fn map_code_to_error_matches_js_default_map() {
        assert!(matches!(
            map_code_to_error(Code::InvalidArgument, "x".into()),
            Error::InvalidArgument(_)
        ));
        assert!(matches!(
            map_code_to_error(Code::Unauthenticated, "x".into()),
            Error::Authentication(_)
        ));
        assert!(matches!(
            map_code_to_error(Code::NotFound, "x".into()),
            Error::NotFound(_)
        ));
        assert!(matches!(
            map_code_to_error(Code::ResourceExhausted, "x".into()),
            Error::RateLimit(_)
        ));
        // Unavailable → sandbox-timeout-formatted Timeout.
        match map_code_to_error(Code::Unavailable, "boom".into()) {
            Error::Timeout(msg) => assert!(msg.contains("sandbox timeout")),
            other => panic!("expected Timeout, got {other:?}"),
        }
        assert!(matches!(
            map_code_to_error(Code::Canceled, "x".into()),
            Error::Timeout(_)
        ));
        assert!(matches!(
            map_code_to_error(Code::DeadlineExceeded, "x".into()),
            Error::Timeout(_)
        ));
        // Anything else → generic Sandbox error.
        assert!(matches!(
            map_code_to_error(Code::Internal, "x".into()),
            Error::Sandbox(_)
        ));
        assert!(matches!(
            map_code_to_error(Code::AlreadyExists, "x".into()),
            Error::Conflict(_)
        ));
    }
}
