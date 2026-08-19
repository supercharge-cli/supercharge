//! Build-log types for the template builder.
//!
//! [`LogEntry`] wraps a single structured log line emitted during a template
//! build. ANSI escape sequences are stripped from the message at construction
//! time so the string is always clean for programmatic use.
//!
//! # Re-exports
//!
//! These types are re-exported at the crate root:
//! - [`LogEntry`]
//! - [`LogEntryLevel`]

use chrono::{DateTime, Utc};

/// Severity level for a [`LogEntry`].
///
/// Mirrors the JSON wire values `debug`, `info`, `warn`, `error` from the E2B
/// API (schema type `crate::api::schema::LogLevel`). Use
/// [`LogEntryLevel::as_str`] to recover the lowercase string representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogEntryLevel {
    /// Verbose diagnostic information.
    Debug,
    /// Informational message about normal progress.
    Info,
    /// Potentially problematic condition that is not an error.
    Warn,
    /// A failure in the build process.
    Error,
}

impl LogEntryLevel {
    /// Returns the lowercase string representation: `"debug"`, `"info"`,
    /// `"warn"`, or `"error"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

impl std::fmt::Display for LogEntryLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single structured log entry emitted by the template build process.
///
/// Produced by the crate-internal `from_wire` constructor from the generated
/// `BuildLogEntry` wire type. The `message` field is guaranteed to be free of
/// ANSI escape sequences.
///
/// # Display format
///
/// Implements [`std::fmt::Display`] with the format:
/// ```text
/// [<rfc3339 timestamp>] [<level>] <message>
/// ```
/// This matches the JavaScript SDK's `LogEntry.toString()`.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Timestamp of when this log line was produced.
    timestamp: DateTime<Utc>,
    /// Severity level of this log entry.
    level: LogEntryLevel,
    /// Cleaned log message (ANSI sequences stripped).
    message: String,
}

impl LogEntry {
    /// Returns the timestamp of this log entry.
    pub fn timestamp(&self) -> DateTime<Utc> {
        self.timestamp
    }

    /// Returns the severity level of this log entry.
    pub fn level(&self) -> LogEntryLevel {
        self.level
    }

    /// Returns the log message with all ANSI escape sequences removed.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Converts the generated wire type [`crate::api::schema::BuildLogEntry`]
    /// into a public [`LogEntry`].
    ///
    /// The `message` is stripped of ANSI/CSI escape sequences so it is safe to
    /// display in terminals that do not support colour codes.
    // Called from types.rs from_wire helpers (Plan 5b/5c callers arrive later).
    #[allow(dead_code)]
    pub(crate) fn from_wire(w: crate::api::schema::BuildLogEntry) -> Self {
        use crate::api::schema::LogLevel;
        let level = match w.level {
            LogLevel::Debug => LogEntryLevel::Debug,
            LogLevel::Info => LogEntryLevel::Info,
            LogLevel::Warn => LogEntryLevel::Warn,
            LogLevel::Error => LogEntryLevel::Error,
        };
        Self {
            timestamp: w.timestamp,
            level,
            message: strip_ansi(&w.message),
        }
    }
}

impl std::fmt::Display for LogEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] [{}] {}",
            // Match the JS SDK's `Date.toISOString()` exactly: 3 fractional-second
            // digits and a `Z` suffix (e.g. `2024-01-01T00:00:00.000Z`), not the
            // default RFC3339 `+00:00` offset without milliseconds.
            self.timestamp
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            self.level,
            self.message
        )
    }
}

/// Removes ANSI/CSI escape sequences from `s`.
///
/// Scans the input byte-by-byte. When an ESC byte (`0x1b`) is encountered:
///
/// - If the next byte is `[` (CSI introducer), the scanner advances until it
///   consumes a *final byte* in the range `0x40..=0x7e` (inclusive), which
///   terminates the CSI sequence (e.g. the `m` in `\x1b[31m`).
/// - Otherwise the lone ESC is dropped.
///
/// All other bytes are passed through unchanged. Valid UTF-8 is preserved
/// because ANSI sequences consist entirely of ASCII bytes; the function
/// operates on raw bytes and re-constructs via [`String::from_utf8_lossy`]
/// (which replaces any ill-formed sequences with `U+FFFD`).
// Called from from_wire (Plan 5b/5c callers arrive later).
#[allow(dead_code)]
fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            // ESC found — check what follows.
            i += 1;
            if i < bytes.len() && bytes[i] == b'[' {
                // CSI sequence: skip parameter bytes until we hit the final byte
                // (0x40–0x7e), which ends the sequence (e.g. 'm' = 0x6d).
                i += 1;
                while i < bytes.len() {
                    let b = bytes[i];
                    i += 1;
                    if (0x40..=0x7e).contains(&b) {
                        break;
                    }
                }
            }
            // Lone ESC (no '[' after it): already incremented past ESC, just
            // discard it and continue.
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::{BuildLogEntry, LogLevel};

    fn fixed_timestamp() -> DateTime<Utc> {
        "2024-01-01T00:00:00Z"
            .parse()
            .expect("valid timestamp literal")
    }

    #[test]
    fn from_wire_maps_level_and_strips_ansi() {
        let wire = BuildLogEntry {
            level: LogLevel::Warn,
            message: "\x1b[31mboom\x1b[0m".to_string(),
            step: None,
            timestamp: fixed_timestamp(),
        };
        let entry = LogEntry::from_wire(wire);
        assert_eq!(entry.level(), LogEntryLevel::Warn);
        assert_eq!(entry.message(), "boom");
    }

    #[test]
    fn display_format() {
        let wire = BuildLogEntry {
            level: LogLevel::Warn,
            message: "\x1b[31mboom\x1b[0m".to_string(),
            step: None,
            timestamp: fixed_timestamp(),
        };
        let entry = LogEntry::from_wire(wire);
        let formatted = format!("{entry}");
        assert!(
            formatted.starts_with('['),
            "expected '[' at start, got: {formatted}"
        );
        assert!(
            formatted.contains("[warn] boom"),
            "expected '[warn] boom' in: {formatted}"
        );
        // The timestamp must match JS `Date.toISOString()` exactly: millisecond
        // precision + `Z` suffix.
        assert_eq!(formatted, "[2024-01-01T00:00:00.000Z] [warn] boom");
    }

    #[test]
    fn strip_ansi_removes_csi_sequences() {
        assert_eq!(strip_ansi("\x1b[31mboom\x1b[0m"), "boom");
    }

    #[test]
    fn strip_ansi_passes_plain_strings() {
        assert_eq!(strip_ansi("plain text"), "plain text");
    }

    #[test]
    fn strip_ansi_drops_lone_esc() {
        // A bare ESC with no '[' after it should be silently dropped.
        assert_eq!(strip_ansi("\x1bhello"), "hello");
    }

    #[test]
    fn strip_ansi_multiple_sequences() {
        // Multiple nested/sequential ANSI codes
        assert_eq!(strip_ansi("\x1b[1m\x1b[32mGreen Bold\x1b[0m"), "Green Bold");
    }

    #[test]
    fn strip_ansi_lone_trailing_esc_no_panic() {
        // A lone ESC at the very end of the string must be dropped without panicking.
        assert_eq!(strip_ansi("hello\x1b"), "hello");
    }

    #[test]
    fn strip_ansi_multibyte_utf8_survives() {
        // Non-ASCII characters (multibyte UTF-8) must pass through unchanged.
        assert_eq!(strip_ansi("\x1b[31mHéllo\x1b[0m"), "Héllo");
    }
}
