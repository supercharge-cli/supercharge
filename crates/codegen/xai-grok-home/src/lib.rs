//! Single source of truth for the Supercharge home directory: `$SUPERCHARGE_HOME`
//! or `<home>/.supercharge`. Shared by the configuration and workspace crates.
//!
//! Which function to call:
//! - [`grok_home`]: the usual choice, a cached, created path to build on.
//! - [`user_grok_home`]: `None` instead of a cwd fallback when no home resolves.
//! - [`default_grok_home`]: the `<home>/.grok` default, ignoring `$GROK_HOME`, so callers can detect an override.
//! - [`resolve_grok_home`]: a fresh, uncached resolve.
//!
//! TODO: collapse these getters by threading the path through config as an
//! explicit value.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// `<home>/.supercharge`, canonicalized via `dunce` (not
/// `std::fs::canonicalize`, which yields Windows `\\?\` verbatim paths).
fn supercharge_home_in(home: &Path) -> PathBuf {
    dunce::canonicalize(home)
        .unwrap_or_else(|_| home.to_path_buf())
        .join(".supercharge")
}

fn legacy_grok_home_in(home: &Path) -> PathBuf {
    dunce::canonicalize(home)
        .unwrap_or_else(|_| home.to_path_buf())
        .join(".grok")
}

fn migrate_default_home(home: &Path) -> PathBuf {
    let current = supercharge_home_in(home);
    let legacy = legacy_grok_home_in(home);
    if !current.exists() && legacy.exists() {
        if let Err(error) = std::fs::rename(&legacy, &current) {
            tracing::warn!(
                from = %legacy.display(),
                to = %current.display(),
                %error,
                "could not migrate legacy home; using it temporarily"
            );
            return legacy;
        }
    }
    current
}

/// `$SUPERCHARGE_HOME` verbatim when non-empty, then legacy `$GROK_HOME`,
/// else `<home>/.supercharge`. Explicit env values are used as-is (not
/// canonicalized) so prefix checks and symlink guards remain stable.
fn resolve_grok_home_from(
    supercharge_home_env: Option<&OsStr>,
    legacy_home_env: Option<&OsStr>,
    os_home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(env) = supercharge_home_env.filter(|env| !env.is_empty()) {
        return Some(PathBuf::from(env));
    }
    if let Some(env) = legacy_home_env.filter(|env| !env.is_empty()) {
        return Some(PathBuf::from(env));
    }
    os_home.map(migrate_default_home)
}

/// Resolve the Supercharge home from the environment (fresh, no cache);
/// `None` if neither home resolves. The legacy variable remains a fallback.
pub fn resolve_grok_home() -> Option<PathBuf> {
    resolve_grok_home_from(
        std::env::var_os("SUPERCHARGE_HOME").as_deref(),
        std::env::var_os("GROK_HOME").as_deref(),
        dirs::home_dir().as_deref(),
    )
}

/// The default `<home>/.supercharge`, used when both home variables are unset.
pub fn default_grok_home() -> PathBuf {
    migrate_default_home(&dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
}

/// The Supercharge home, created if missing and cached for the process; falls
/// back to [`default_grok_home`] when neither home variable nor OS home resolves.
pub fn grok_home() -> PathBuf {
    static GROK_HOME: OnceLock<PathBuf> = OnceLock::new();
    GROK_HOME
        .get_or_init(|| {
            let home = resolve_grok_home().unwrap_or_else(default_grok_home);
            if let Err(err) = std::fs::create_dir_all(&home) {
                tracing::warn!(path = %home.display(), %err, "failed to create Supercharge home");
            }
            home
        })
        .clone()
}

/// Like [`grok_home`], but `None` when no home resolves (no cwd fallback).
pub fn user_grok_home() -> Option<PathBuf> {
    resolve_grok_home().is_some().then(grok_home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::ffi::OsString;

    #[test]
    fn env_wins_over_os_home() {
        let resolved = resolve_grok_home_from(
            Some(OsStr::new("/custom/home")),
            None,
            Some(Path::new("/home/u")),
        );
        assert_eq!(resolved, Some(PathBuf::from("/custom/home")));
    }

    #[test]
    fn supercharge_env_wins_over_legacy_env() {
        let resolved = resolve_grok_home_from(
            Some(OsStr::new("/custom/supercharge")),
            Some(OsStr::new("/custom/legacy")),
            Some(Path::new("/home/u")),
        );
        assert_eq!(resolved, Some(PathBuf::from("/custom/supercharge")));
    }

    #[test]
    fn env_used_verbatim_even_when_it_exists() {
        // A real, existing dir whose canonical form differs (macOS symlinks
        // `/var` -> `/private/var`): the env value must come back unchanged.
        let tmp = tempfile::tempdir().unwrap();
        let resolved = resolve_grok_home_from(Some(tmp.path().as_os_str()), None, None);
        assert_eq!(resolved, Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn empty_env_falls_through_to_os_home() {
        let tmp = tempfile::tempdir().unwrap();
        let resolved = resolve_grok_home_from(Some(&OsString::new()), None, Some(tmp.path()));
        assert_eq!(
            resolved,
            Some(
                dunce::canonicalize(tmp.path())
                    .unwrap()
                    .join(".supercharge")
            )
        );
    }

    #[test]
    fn legacy_home_is_migrated_to_supercharge() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join(".grok");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("config.toml"), "[ui]\n").unwrap();

        let current = migrate_default_home(tmp.path());

        assert!(current.ends_with(".supercharge"));
        assert!(current.join("config.toml").is_file());
        assert!(!legacy.exists());
    }

    #[test]
    fn default_grok_home_has_no_verbatim_prefix() {
        // The reason we canonicalize via dunce: std::fs::canonicalize yields
        // `\\?\` verbatim paths on Windows that break git and byte-exact
        // comparisons. No-op assertion on Unix.
        let home = default_grok_home();
        assert!(!home.to_string_lossy().starts_with(r"\\?\"));
        assert!(home.ends_with(".supercharge"));
    }

    #[test]
    fn none_when_nothing_resolves() {
        assert_eq!(
            resolve_grok_home_from(
                /* supercharge_home_env */ None, /* legacy_home_env */ None,
                /* os_home */ None
            ),
            None
        );
    }
}
