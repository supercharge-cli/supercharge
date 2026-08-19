//! envd feature-gate versions (port of `envd/versions.ts`).

/// Recursive directory watch (`recursive` on watchDir).
#[allow(dead_code)] // used by Task 4 / Plan 3
pub(crate) const ENVD_VERSION_RECURSIVE_WATCH: &str = "0.1.4";
/// `stdin` option on command start.
#[allow(dead_code)] // used by Task 4 / Plan 3
pub(crate) const ENVD_COMMANDS_STDIN: &str = "0.3.0";
/// Default-user (Basic auth) support.
#[allow(dead_code)] // used by Task 4 / Plan 3
pub(crate) const ENVD_DEFAULT_USER: &str = "0.4.0";
/// `closeStdin` RPC.
#[allow(dead_code)] // used by Task 4 / Plan 3
pub(crate) const ENVD_ENVD_CLOSE: &str = "0.5.2";
/// octet-stream uploads + gzip.
#[allow(dead_code)] // used by Task 4 / Plan 3
pub(crate) const ENVD_OCTET_STREAM_UPLOAD: &str = "0.5.7";
/// File metadata (xattr) on write.
#[allow(dead_code)] // used by Task 4 / Plan 3
pub(crate) const ENVD_FILE_METADATA: &str = "0.6.2";
/// `includeEntry` in filesystem watch events.
#[allow(dead_code)] // used by Task 4 / Plan 3
pub(crate) const ENVD_VERSION_FS_EVENT_ENTRY_INFO: &str = "0.6.3";
/// `allowNetworkMounts` on watch.
#[allow(dead_code)] // used by Task 4 / Plan 3
pub(crate) const ENVD_VERSION_WATCH_NETWORK_MOUNTS: &str = "0.6.4";

/// Return `true` if `actual >= required` (both semver). An unparseable `actual`
/// returns `false` (treated as too old) rather than panicking; `required` is
/// one of the constants above and is always valid.
#[allow(dead_code)] // used by Task 4 / Plan 3
pub(crate) fn version_gte(actual: &str, required: &str) -> bool {
    // Strip a leading `v` if present, then parse.
    let parse = |s: &str| semver::Version::parse(s.trim_start_matches('v'));
    match (parse(actual), parse(required)) {
        (Ok(a), Ok(r)) => a >= r,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_constants_match_js() {
        assert_eq!(ENVD_DEFAULT_USER, "0.4.0");
        assert_eq!(ENVD_ENVD_CLOSE, "0.5.2");
        assert_eq!(ENVD_OCTET_STREAM_UPLOAD, "0.5.7");
    }

    #[test]
    fn version_gte_compares_semver() {
        assert!(version_gte("0.4.0", "0.4.0"));
        assert!(version_gte("0.5.2", "0.4.0"));
        assert!(version_gte("1.0.0", "0.6.4"));
        assert!(!version_gte("0.3.9", "0.4.0"));
        // Unparseable actual is treated as "too old" (false), never panics.
        assert!(!version_gte("not-a-version", "0.4.0"));
    }
}
