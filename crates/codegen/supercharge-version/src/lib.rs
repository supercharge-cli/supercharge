//! Installed grok CLI version, kept in sync with the shipping binaries.

#![deny(clippy::indexing_slicing)]

use std::sync::OnceLock;

use semver::Version;

pub const TEST_VERSION_ENV: &str = "SUPERCHARGE_TEST_VERSION";

pub const VERSION: &str = match option_env!("SUPERCHARGE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// The release pipeline always injects `SUPERCHARGE_VERSION`; without it the build is from source.
pub const IS_DEV_BUILD: bool = option_env!("SUPERCHARGE_VERSION").is_none();

/// Runtime-injected `"<version> (<shortcommit>)"` string.
/// Only the release binary stamps the commit in its own build.rs and injects it here at startup, so the lib crates don't recompile on every commit.
static FULL_VERSION: OnceLock<&'static str> = OnceLock::new();

/// Inject the binary's stamped `"<version> (<shortcommit>)"` string.
/// Idempotent: the first set wins, repeats are ignored.
pub fn set_full_version(v: &'static str) {
    let _ = FULL_VERSION.set(v);
}

/// The injected version-with-commit string, or plain [`VERSION`] when no binary has called [`set_full_version`] (e.g. lib tests, dev harnesses).
pub fn full_version() -> &'static str {
    FULL_VERSION.get().copied().unwrap_or(VERSION)
}

/// Returns the [`TEST_VERSION_ENV`] override when set, otherwise [`VERSION`].
/// The env value is trimmed so non-semver-aware callers can pass the result straight into parsing.
pub fn installed() -> String {
    std::env::var(TEST_VERSION_ENV)
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|_| VERSION.to_string())
}

pub fn installed_semver() -> Result<Version, semver::Error> {
    Version::parse(&installed())
}

/// Formats the compiled version with a channel label for user-facing display, e.g. `"0.2.5 [stable]"`.
/// `channel_label` is pre-formatted by `supercharge_update::channel_label()`: `" [alpha]"`, `" [stable]"`, or `""` when no pointer is cached.
pub fn display_version(channel_label: &str) -> String {
    format!("{}{}", VERSION, channel_label)
}

/// Like [`display_version`], but for the full `"0.2.5 (abc1234)"` string.
pub fn display_version_with_commit(version_with_commit: &str, channel_label: &str) -> String {
    format!("{}{}", version_with_commit, channel_label)
}

/// Parse the package version from a CLI `--version` line such as
/// `supercharge 1.4.15 (dd042d764dd2)` or `supercharge 1.4.15 (dd042d764dd2) [stable]`.
pub fn parse_cli_version_line(output: &str) -> Option<&str> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let rest = line.strip_prefix("supercharge ")?.trim_start();
    let version = rest.split_whitespace().next()?;
    if version.is_empty() {
        return None;
    }
    Some(version)
}

fn normalize_advertised_version(advertised: &str) -> &str {
    let trimmed = advertised.trim();
    trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed)
}

/// True when `--version` output reports the same package version as the advertised release.
pub fn cli_version_matches_advertised(output: &str, advertised: &str) -> bool {
    match parse_cli_version_line(output) {
        Some(got) => got == normalize_advertised_version(advertised),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_version_line_reads_package_version() {
        assert_eq!(
            parse_cli_version_line("supercharge 1.4.15 (dd042d764dd2)\n"),
            Some("1.4.15")
        );
        assert_eq!(
            parse_cli_version_line("supercharge 1.4.9 (dd042d764dd2) [stable]\n"),
            Some("1.4.9")
        );
        assert_eq!(
            parse_cli_version_line("\n  supercharge 1.4.19 (abcdef012345)\n"),
            Some("1.4.19")
        );
        assert_eq!(parse_cli_version_line("not a version"), None);
        assert_eq!(parse_cli_version_line(""), None);
    }

    #[test]
    fn cli_version_matches_advertised_rejects_release_tag_mismatch() {
        let claimed_1415_asset = "supercharge 1.4.9 (dd042d764dd2)\n";
        assert!(
            !cli_version_matches_advertised(claimed_1415_asset, "1.4.15"),
            "v1.4.15 assets must not be accepted when the binary reports 1.4.9"
        );
        assert!(!cli_version_matches_advertised(
            claimed_1415_asset,
            "v1.4.15"
        ));
        assert!(cli_version_matches_advertised(claimed_1415_asset, "1.4.9"));
        assert!(cli_version_matches_advertised(
            "supercharge 1.4.19 (abc012345678)\n",
            "v1.4.19"
        ));
        assert!(!cli_version_matches_advertised("supercharge\n", "1.4.19"));
    }

    #[test]
    fn cli_version_matches_advertised_uses_exact_version_boundary() {
        assert!(
            !cli_version_matches_advertised("supercharge 1.4.15 (dd042d764dd2)\n", "1.4.1"),
            "1.4.15 must not satisfy advertised 1.4.1"
        );
        assert!(!cli_version_matches_advertised(
            "supercharge 1.4.1 (dd042d764dd2)\n",
            "1.4.15"
        ));
        assert!(!cli_version_matches_advertised(
            "supercharge 1.4.19 (abcdef012345)\n",
            "1.4.1"
        ));
        assert!(!cli_version_matches_advertised(
            "supercharge 1.4.19 (abcdef012345)\n",
            "1.4.9"
        ));
        assert!(!cli_version_matches_advertised("", "1.4.19"));
        assert!(!cli_version_matches_advertised("ok\n", "1.4.19"));
        assert!(!cli_version_matches_advertised("supercharge\n", "1.4.19"));
    }

    /// Checks that the channel label is appended for alpha, stable, and empty labels.
    #[test]
    fn test_display_version_formatting_matrix() {
        let cases: &[(&str, &str, &str)] = &[
            // (version_with_commit,    label,        expected_suffix)
            ("0.2.5 (abc1234)", " [alpha]", "0.2.5 (abc1234) [alpha]"),
            ("0.2.5 (abc1234)", " [stable]", "0.2.5 (abc1234) [stable]"),
            ("0.2.5 (abc1234)", "", "0.2.5 (abc1234)"),
            (
                "0.1.220-alpha.2 (def0)",
                " [alpha]",
                "0.1.220-alpha.2 (def0) [alpha]",
            ),
        ];
        for (vwc, label, expected) in cases {
            assert_eq!(
                display_version_with_commit(vwc, label),
                *expected,
                "display_version_with_commit({:?}, {:?})",
                vwc,
                label,
            );
        }
        // display_version uses compiled VERSION, so verify only that the label appends
        assert_eq!(display_version(""), VERSION);
        assert!(display_version(" [stable]").ends_with("[stable]"));
    }

    #[test]
    fn full_version_falls_back_then_first_set_wins() {
        assert_eq!(full_version(), VERSION);
        set_full_version("first (aaaaaaa)");
        assert_eq!(full_version(), "first (aaaaaaa)");
        set_full_version("second (bbbbbbb)");
        assert_eq!(full_version(), "first (aaaaaaa)");
    }
}
