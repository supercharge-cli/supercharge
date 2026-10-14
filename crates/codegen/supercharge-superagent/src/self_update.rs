//! Opt-in E2B self-update for Supercharge (not XibeCode).
//! Downloads linux-x86_64 daemon + CLI from GitHub Releases (or a custom
//! base URL) and relaunches the daemon. Session JSON under /var/lib/superagent
//! is kept.

use std::{env, path::Path, time::Duration};

use serde::{Deserialize, Serialize};

const DEFAULT_REPO: &str = "supercharge-cli/supercharge-releases";
const DAEMON_ASSET: &str = "supercharge-superagent-server-linux-x86_64";
const CLI_ASSET: &str = "supercharge-linux-x86_64";
const VERSION_STAMP: &str = "/var/lib/superagent/e2b-version";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateAction {
    Check,
    Apply,
    Dismiss,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    version: Option<String>,
    #[serde(default)]
    daemon: Option<String>,
    #[serde(default)]
    cli: Option<String>,
}

fn update_repo() -> String {
    env::var("SUPERCHARGE_E2B_UPDATE_REPO")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_REPO.to_owned())
}

fn update_base_url() -> String {
    env::var("SUPERCHARGE_E2B_UPDATE_BASE_URL")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            format!(
                "https://github.com/{}/releases/latest/download",
                update_repo()
            )
        })
}

fn current_version() -> String {
    if let Ok(value) = std::fs::read_to_string(VERSION_STAMP) {
        let value = value.trim().to_owned();
        if !value.is_empty() {
            return value;
        }
    }
    env::var("SUPERCHARGE_E2B_IMAGE_VERSION")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned())
}

async fn fetch_manifest() -> Result<Manifest, String> {
    let url = format!("{}/e2b-manifest.json", update_base_url());
    let client = crate::http_client(|builder| {
        builder
            .timeout(Duration::from_secs(20))
            .user_agent("supercharge-e2b-update")
    })
    .map_err(|error| error.to_string())?;
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Ok(Manifest {
            version: Some("latest".to_owned()),
            daemon: Some(format!("{}/{}", update_base_url(), DAEMON_ASSET)),
            cli: Some(format!("{}/{}", update_base_url(), CLI_ASSET)),
        });
    }
    response.json().await.map_err(|error| error.to_string())
}

fn asset_url(manifest: &Manifest, kind: &str) -> String {
    let explicit = if kind == "daemon" {
        manifest.daemon.as_deref()
    } else {
        manifest.cli.as_deref()
    };
    if let Some(url) = explicit.filter(|value| value.starts_with("http")) {
        return url.to_owned();
    }
    let name = if kind == "daemon" {
        DAEMON_ASSET
    } else {
        CLI_ASSET
    };
    format!("{}/{}", update_base_url(), name)
}

pub async fn handle(action: UpdateAction) -> String {
    match action {
        UpdateAction::Dismiss => {
            "OK — skipped Supercharge update. Ask again with `/update`.".to_owned()
        }
        UpdateAction::Check => match fetch_manifest().await {
            Ok(manifest) => {
                let latest = manifest
                    .version
                    .clone()
                    .unwrap_or_else(|| "latest".to_owned());
                let current = current_version();
                let available = latest != current;
                format!(
                    "## Supercharge update (E2B)\n\n\
- Running: `{current}`\n\
- Latest: `{latest}`\n\
- Feed: `{}`\n\n\
{}\n\n\
Reply **`/update yes`** to download the Supercharge daemon + CLI into this VM and restart (chats kept). **`/update no`** to dismiss.\n\n\
This does **not** rebuild the E2B template. XibeCode is not touched.",
                    update_base_url(),
                    if available {
                        "An update is available."
                    } else {
                        "You already look up to date. `/update yes` still re-fetches the published binaries."
                    }
                )
            }
            Err(error) => format!(
                "## Supercharge update\n\nCould not check the feed (`{}`):\n\n```\n{error}\n```\n\nPublish binaries with `sandbox/publish-e2b-update.sh`, or set `SUPERCHARGE_E2B_UPDATE_BASE_URL`.",
                update_base_url()
            ),
        },
        UpdateAction::Apply => {
            if !is_e2b_runtime() {
                return "Supercharge `/update yes` only runs inside an E2B sandbox (Vectra Cloud). It does not touch XibeCode.".to_owned();
            }
            apply_update().await
        }
    }
}

async fn apply_update() -> String {
    match crate::browser::desktop_status().await {
        Ok(status) if status.desktop_protocol > 0 || status.handoff_phase == "active" => {
            return "This sandbox uses coordinated desktop control. The legacy in-chat updater cannot safely restart its concurrent agents or browser. Use an explicit idle-sandbox maintenance update; no files or processes were changed.".to_owned();
        }
        Err(_) => return "Cannot verify desktop/handoff state. The update was not started; retry when the runtime is healthy.".to_owned(),
        _ => {}
    }
    let manifest = match fetch_manifest().await {
        Ok(manifest) => manifest,
        Err(error) => return format!("Update check failed: {error}"),
    };
    let latest = manifest
        .version
        .clone()
        .unwrap_or_else(|| "latest".to_owned());
    let from = current_version();
    let daemon_url = asset_url(&manifest, "daemon");
    let cli_url = asset_url(&manifest, "cli");
    let output = tokio::process::Command::new("bash")
        .arg("-c")
        .arg(
            r#"
set -euo pipefail
TMP=/tmp/supercharge-e2b-update
mkdir -p "$TMP" /var/lib/superagent
echo "--- download daemon ---"
curl -fL --retry 3 -o "$TMP/supercharge-superagent-server" "$SC_DAEMON_URL"
echo "--- download cli ---"
curl -fL --retry 3 -o "$TMP/supercharge" "$SC_CLI_URL"
chmod +x "$TMP/supercharge-superagent-server" "$TMP/supercharge"
test -s "$TMP/supercharge-superagent-server"
test -s "$TMP/supercharge"
echo "--- install ---"
sudo -n install -m 0755 "$TMP/supercharge-superagent-server" /usr/local/bin/supercharge-superagent-server
sudo -n install -m 0755 "$TMP/supercharge" /usr/local/bin/supercharge
sudo -n ln -sfn /usr/local/bin/supercharge /usr/local/bin/sc
printf '%s\n' "$SC_VERSION" > /var/lib/superagent/e2b-version || true
echo INSTALLED
/usr/local/bin/supercharge --version 2>/dev/null || true
"#,
        )
        .env("SC_DAEMON_URL", daemon_url)
        .env("SC_CLI_URL", cli_url)
        .env("SC_VERSION", &latest)
        .output()
        .await;
    match output {
        Ok(output) => {
            let log = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if !output.status.success() || !log.contains("INSTALLED") {
                return format!(
                    "## Supercharge update failed\n\nFrom `{from}` toward `{latest}`.\n\n```\n{}\n```",
                    cap_log(&log)
                );
            }
            schedule_relaunch();
            format!(
                "## Supercharge updated\n\n- From: `{from}`\n- To: `{latest}`\n- Chats kept under `/var/lib/superagent`\n- Daemon restarting in ~2s\n\n```\n{}\n```",
                cap_log(&log)
            )
        }
        Err(error) => format!("Could not run updater: {error}"),
    }
}

fn cap_log(log: &str) -> String {
    let trimmed = log.trim();
    if trimmed.chars().count() <= 3500 {
        trimmed.to_owned()
    } else {
        trimmed.chars().take(3499).collect::<String>() + "…"
    }
}

fn schedule_relaunch() {
    let command = env::var("SUPERAGENT_DAEMON_COMMAND")
        .unwrap_or_else(|_| "/usr/local/bin/supercharge-superagent-server".to_owned());
    let script = format!(
        r#"
sleep 2
if [ -f /tmp/supercharge-daemon.pid ]; then
  kill "$(cat /tmp/supercharge-daemon.pid)" 2>/dev/null || true
fi
pkill -f 'supercharge-superagent-server' 2>/dev/null || true
sleep 1
nohup {command} >>/tmp/supercharge-daemon.log 2>&1 &
echo $! > /tmp/supercharge-daemon.pid
"#
    );
    #[allow(clippy::disallowed_methods)]
    // detached self-update relaunch intentionally outlives this daemon
    let _ = std::process::Command::new("bash")
        .arg("-c")
        .arg(script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

pub fn is_e2b_runtime() -> bool {
    env::var("SUPERCHARGE_RUNTIME_MODE")
        .ok()
        .or_else(|| env::var("SUPERAGENT_RUNTIME_MODE").ok())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("e2b"))
        || Path::new("/run/e2b/.E2B_SANDBOX_ID").exists()
        || env::var("E2B_SANDBOX_ID")
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::{CLI_ASSET, DAEMON_ASSET, DEFAULT_REPO};

    #[test]
    fn default_feed_points_at_supercharge_github_releases() {
        assert!(DEFAULT_REPO.contains("supercharge"));
        assert!(DAEMON_ASSET.contains("superagent-server"));
        assert!(CLI_ASSET.starts_with("supercharge-"));
    }
}
