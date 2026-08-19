//! Signed-URL signatures for sandbox file access.

use crate::errors::{Error, Result};
use crate::utils::sha256_base64;

/// File-system operation a signature authorizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureOperation {
    /// Read access.
    Read,
    /// Write access.
    Write,
}

impl SignatureOperation {
    fn as_str(self) -> &'static str {
        match self {
            SignatureOperation::Read => "read",
            SignatureOperation::Write => "write",
        }
    }
}

/// A computed URL signature and its absolute expiration (unix seconds, if any).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// The `v1_`-prefixed signature value.
    pub signature: String,
    /// Absolute expiration as a unix timestamp in seconds, or `None` if it never expires.
    pub expiration: Option<i64>,
}

/// Compute a signature for accessing `path` with `operation` as `user`.
///
/// Mirrors the JS `getSignature`. When `expiration_in_seconds` is provided it is
/// added to `now_unix` to form an absolute expiration. `now_unix` is injected so
/// callers (and tests) control the clock; see [`get_signature_now`] for the
/// convenience wrapper that uses the system clock.
///
/// # Errors
/// Returns [`Error::Sandbox`] if `envd_access_token` is missing or empty.
pub fn get_signature(
    path: &str,
    operation: SignatureOperation,
    user: Option<&str>,
    expiration_in_seconds: Option<i64>,
    envd_access_token: Option<&str>,
    now_unix: i64,
) -> Result<Signature> {
    let token = match envd_access_token {
        Some(t) if !t.is_empty() => t,
        _ => {
            return Err(Error::Sandbox(
                "Access token is not set and signature cannot be generated!".to_string(),
            ));
        }
    };

    let user = user.unwrap_or("");
    let expiration = expiration_in_seconds.map(|secs| now_unix + secs);
    let op = operation.as_str();

    let raw = match expiration {
        None => format!("{path}:{op}:{user}:{token}"),
        Some(exp) => format!("{path}:{op}:{user}:{token}:{exp}"),
    };

    let hash = sha256_base64(&raw);
    let signature = format!("v1_{}", hash.trim_end_matches('='));

    Ok(Signature {
        signature,
        expiration,
    })
}

/// Like [`get_signature`] but reads the current system time for expiration.
///
/// # Errors
/// Returns [`Error::Sandbox`] if `envd_access_token` is missing or empty.
pub fn get_signature_now(
    path: &str,
    operation: SignatureOperation,
    user: Option<&str>,
    expiration_in_seconds: Option<i64>,
    envd_access_token: Option<&str>,
) -> Result<Signature> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    get_signature(
        path,
        operation,
        user,
        expiration_in_seconds,
        envd_access_token,
        now,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::sha256_base64;

    #[test]
    fn missing_token_errors() {
        let err = get_signature("/f", SignatureOperation::Read, None, None, None, 0).unwrap_err();
        assert!(matches!(err, crate::errors::Error::Sandbox(_)));
    }

    #[test]
    fn unexpiring_signature_matches_assembly() {
        let sig =
            get_signature("/f", SignatureOperation::Read, None, None, Some("tok"), 0).unwrap();
        let expected_hash = sha256_base64("/f:read::tok");
        let expected = format!("v1_{}", expected_hash.trim_end_matches('='));
        assert_eq!(sig.signature, expected);
        assert_eq!(sig.expiration, None);
        assert!(sig.signature.starts_with("v1_"));
        assert!(!sig.signature.ends_with('='));
    }

    #[test]
    fn expiring_signature_adds_offset_to_now() {
        let sig = get_signature(
            "/f",
            SignatureOperation::Write,
            Some("alice"),
            Some(100),
            Some("tok"),
            1000,
        )
        .unwrap();
        assert_eq!(sig.expiration, Some(1100));
        let expected_hash = sha256_base64("/f:write:alice:tok:1100");
        assert_eq!(
            sig.signature,
            format!("v1_{}", expected_hash.trim_end_matches('='))
        );
    }
}
