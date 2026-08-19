//! Internal utility functions ported from the JS SDK's `utils.ts`.

use base64::prelude::{BASE64_STANDARD, Engine as _};
use sha2::{Digest, Sha256};

/// SHA-256 of `data`, encoded as standard base64 (with `=` padding), matching
/// the JS `sha256` helper.
pub(crate) fn sha256_base64(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    BASE64_STANDARD.encode(hasher.finalize())
}

/// Convert milliseconds to whole seconds, rounding up (JS `timeoutToSeconds`).
#[allow(dead_code)]
pub(crate) fn timeout_to_seconds(ms: u64) -> u64 {
    ms.div_ceil(1000)
}

/// True for characters Python's `shlex.quote` leaves unquoted: `[A-Za-z0-9_]`
/// plus `@%+=:,./-`.
#[allow(dead_code)]
fn is_safe_shell_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '_' | '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '-')
}

/// Quote a string for safe interpolation into a POSIX shell command. Faithful
/// port of Python's `shlex.quote` (the JS `shellQuote`): empty becomes `''`,
/// all-safe strings are returned unchanged, otherwise single-quote-wrapped with
/// embedded single quotes escaped as `'"'"'`.
#[allow(dead_code)]
pub(crate) fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.chars().all(is_safe_shell_char) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// Build the `User-Agent` header value, optionally tagging an integration.
pub(crate) fn build_user_agent(integration: Option<&str>) -> String {
    let base = concat!("e2b-rs/", env!("CARGO_PKG_VERSION"));
    match integration {
        Some(name) if !name.is_empty() => format!("{base} {name}"),
        _ => base.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_base64_known_vectors() {
        // Standard test vectors for SHA-256, base64-encoded with padding.
        assert_eq!(
            sha256_base64(""),
            "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
        );
        assert_eq!(
            sha256_base64("abc"),
            "ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0="
        );
    }

    #[test]
    fn timeout_to_seconds_rounds_up() {
        assert_eq!(timeout_to_seconds(0), 0);
        assert_eq!(timeout_to_seconds(1), 1);
        assert_eq!(timeout_to_seconds(1000), 1);
        assert_eq!(timeout_to_seconds(1001), 2);
        assert_eq!(timeout_to_seconds(300_000), 300);
    }

    #[test]
    fn shell_quote_matches_shlex() {
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("abc"), "abc");
        assert_eq!(shell_quote("a_b.c-d/e@f"), "a_b.c-d/e@f");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("$x"), "'$x'");
        // Embedded single quote becomes '"'"'
        assert_eq!(shell_quote("it's"), "'it'\"'\"'s'");
    }

    #[test]
    fn user_agent_contains_version() {
        let v = env!("CARGO_PKG_VERSION");
        assert_eq!(build_user_agent(None), format!("e2b-rs/{v}"));
        assert_eq!(
            build_user_agent(Some("langchain")),
            format!("e2b-rs/{v} langchain")
        );
        assert_eq!(build_user_agent(Some("")), format!("e2b-rs/{v}")); // empty = falsy, like JS
    }
}
