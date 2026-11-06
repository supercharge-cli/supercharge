//! Download / status / uninstall for the optional local browser binary.

use std::path::{Path, PathBuf};

use supercharge_dirs::supercharge_home;

const RELEASES_API: &str = "https://api.github.com/repos/h4ckf0r0day/obscura/releases/latest";
const USER_AGENT: &str = "supercharge-browser-installer";

#[derive(Debug, thiserror::Error)]
pub enum BrowserInstallError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

/// `$SUPERCHARGE_HOME/browser/`
pub fn browser_dir() -> PathBuf {
    supercharge_home().join("browser")
}

pub fn binary_path() -> PathBuf {
    let dir = browser_dir();
    if cfg!(windows) {
        dir.join("obscura.exe")
    } else {
        dir.join("obscura")
    }
}

pub fn version_path() -> PathBuf {
    browser_dir().join("VERSION")
}

pub fn is_installed() -> bool {
    binary_path().is_file()
}

pub fn status_report() -> String {
    let path = binary_path();
    if !path.is_file() {
        return format!(
            "browser: not installed (expected {})\nplatforms with release binaries: {}",
            path.display(),
            supported_platforms_summary()
        );
    }
    let version = std::fs::read_to_string(version_path()).unwrap_or_else(|_| "unknown".into());
    format!(
        "browser: installed\n  path: {}\n  version: {}\n  platforms with release binaries: {}",
        path.display(),
        version.trim(),
        supported_platforms_summary()
    )
}

pub fn supported_platforms_summary() -> &'static str {
    "linux-x86_64, linux-aarch64, macos-x86_64, macos-aarch64, windows-x86_64 (no windows-aarch64 asset)"
}

/// Asset stem for the current host, without extension. `None` when this platform
/// has no published binary.
pub fn asset_stem_for_host() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("obscura-x86_64-linux"),
        ("linux", "aarch64") => Some("obscura-aarch64-linux"),
        ("macos", "x86_64") => Some("obscura-x86_64-macos"),
        ("macos", "aarch64") => Some("obscura-aarch64-macos"),
        ("windows", "x86_64") => Some("obscura-x86_64-windows"),
        _ => None,
    }
}

fn archive_name(stem: &str) -> String {
    if stem.contains("windows") {
        format!("{stem}.zip")
    } else {
        format!("{stem}.tar.gz")
    }
}

pub fn uninstall() -> Result<(), BrowserInstallError> {
    let dir = browser_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

pub async fn install_latest() -> Result<PathBuf, BrowserInstallError> {
    let Some(stem) = asset_stem_for_host() else {
        return Err(BrowserInstallError::Message(format!(
            "no browser binary is published for {}-{}; available: {}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            supported_platforms_summary()
        )));
    };
    let archive = archive_name(stem);
    let client = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
    let release: serde_json::Value = client
        .get(RELEASES_API)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let tag = release
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let assets = release
        .get("assets")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let asset = assets.iter().find(|a| {
        a.get("name")
            .and_then(|n| n.as_str())
            .is_some_and(|n| n == archive)
    });
    let Some(asset) = asset else {
        return Err(BrowserInstallError::Message(format!(
            "release {tag} has no asset named {archive}; falling back unavailable for this platform"
        )));
    };
    let url = asset
        .get("browser_download_url")
        .and_then(|u| u.as_str())
        .ok_or_else(|| BrowserInstallError::Message("asset missing download URL".into()))?;

    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let dir = browser_dir();
    std::fs::create_dir_all(&dir)?;
    let archive_path = dir.join(&archive);
    std::fs::write(&archive_path, &bytes)?;
    extract_archive(&archive_path, &dir)?;
    let _ = std::fs::remove_file(&archive_path);

    // Archives unpack `obscura` (and sometimes `obscura-worker`) into the cwd we extract into.
    let dest = binary_path();
    if !dest.is_file() {
        // Some archives nest one directory.
        if let Some(found) = find_binary(&dir) {
            if found != dest {
                std::fs::rename(&found, &dest)?;
            }
        }
    }
    if !dest.is_file() {
        return Err(BrowserInstallError::Message(format!(
            "archive extracted but binary not found at {}",
            dest.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms)?;
    }
    std::fs::write(version_path(), format!("{tag}\n"))?;
    let _ = write_bundled_skill();
    Ok(dest)
}

fn find_binary(dir: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "obscura.exe"
    } else {
        "obscura"
    };
    for entry in walkdir_shallow(dir) {
        if entry.file_name().is_some_and(|n| n == name) && entry.is_file() {
            return Some(entry);
        }
    }
    None
}

fn walkdir_shallow(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_file() {
            out.push(path);
        } else if path.is_dir() {
            if let Ok(nested) = std::fs::read_dir(&path) {
                for n in nested.flatten() {
                    out.push(n.path());
                }
            }
        }
    }
    out
}

fn extract_archive(archive: &Path, dest: &Path) -> Result<(), BrowserInstallError> {
    let name = archive.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let status = if name.ends_with(".zip") {
        std::process::Command::new("unzip")
            .args(["-o", "-q"])
            .arg(archive)
            .arg("-d")
            .arg(dest)
            .status()?
    } else {
        std::process::Command::new("tar")
            .args(["-xzf"])
            .arg(archive)
            .arg("-C")
            .arg(dest)
            .status()?
    };
    if !status.success() {
        return Err(BrowserInstallError::Message(format!(
            "archive extract failed with {status}"
        )));
    }
    Ok(())
}

const BROWSER_SKILL: &str = r#"---
name: browser
description: Use the local headless browser for interactive pages; prefer web_fetch for static HTML.
---

# When to use the browser vs web fetch

- Prefer **web_fetch** (or plain HTTP) for static documentation, READMEs, and API JSON.
- Use the **browser** when the page needs JavaScript, login flows, clicks, typing, or a screenshot.
- Keep the browser session short: navigate, extract what you need, then stop.
- Never use the browser for `file://` URLs that reach outside the workspace you were asked about.

# Callable tools

These tools exist only after `supercharge browser install`. They are distinct from the cloud `browser_*` tools.

- `local_browser_open` — open or navigate to a URL
- `local_browser_read` — read visible page text
- `local_browser_click` — click a CSS selector
- `local_browser_type` — type into a CSS selector
- `local_browser_screenshot` — capture a PNG screenshot
- `local_browser_evaluate` — run JavaScript and return the value
"#;

/// Write the bundled browser skill under `$SUPERCHARGE_HOME/skills/browser/`.
pub fn write_bundled_skill() -> Result<PathBuf, BrowserInstallError> {
    let dir = supercharge_home().join("skills").join("browser");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("SKILL.md");
    std::fs::write(&path, BROWSER_SKILL)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_stem_covers_current_host_or_none() {
        // Just ensures the match arms compile and return something coherent.
        let _ = asset_stem_for_host();
        assert!(supported_platforms_summary().contains("linux-x86_64"));
    }

    #[test]
    fn bundled_skill_names_callable_local_browser_tools() {
        for name in crate::browser::LOCAL_BROWSER_TOOL_IDS {
            assert!(
                BROWSER_SKILL.contains(&format!("`{name}`")),
                "skill missing `{name}`"
            );
        }
        assert!(!BROWSER_SKILL.contains("`browser_navigate`"));
        assert!(!BROWSER_SKILL.contains("`browser_screenshot`"));
    }
}
