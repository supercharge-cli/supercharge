//! `/teleport`: push this workspace to an E2B sandbox, pull a bundle back, or kill it.
//!
//! Teleport v1 is a workspace handoff, not a live session sync:
//! - `push` packs every non-ignored file (tracked + untracked) at the session cwd into a
//!   tarball, uploads it to a fresh sandbox, extracts it, and hands back the sandbox id.
//! - `pull <id>` tars the sandbox workspace and stores it under `<cwd>/.supercharge/teleport/`.
//! - `kill <id>` stops the sandbox.
//!
//! Compiled only with the shell's `teleport` feature (plies supercharge-superagent's `e2b`).

use std::path::Path;

/// Largest tarball we will upload (the envd file upload is not meant for gigabytes).
const MAX_PACK_BYTES: u64 = 256 * 1024 * 1024;
/// In-sandbox extraction root.
const SANDBOX_DIR: &str = "/home/user/teleport";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TeleportOp {
    Push { template: Option<String> },
    Pull { sandbox_id: String },
    Kill { sandbox_id: String },
    Help,
}

impl TeleportOp {
    /// Parse the `/teleport` argument tail. A bare word is a sandbox template name.
    pub(crate) fn parse(args: &str) -> Self {
        let args = args.trim();
        let (sub, rest) = match args.split_once(char::is_whitespace) {
            Some((sub, rest)) => (sub, rest.trim()),
            None => (args, ""),
        };
        match sub {
            "" => Self::Push { template: None },
            "push" => Self::Push {
                template: (!rest.is_empty()).then(|| rest.to_string()),
            },
            "pull" if !rest.is_empty() => Self::Pull {
                sandbox_id: rest.to_string(),
            },
            "kill" | "stop" if !rest.is_empty() => Self::Kill {
                sandbox_id: rest.to_string(),
            },
            // Bare word: a sandbox template, e.g. `/teleport rust-build-16` —
            // except a lone subcommand keyword, which misses its id.
            "pull" | "kill" | "stop" => Self::Help,
            _ if rest.is_empty() => Self::Push {
                template: Some(sub.to_string()),
            },
            _ => Self::Help,
        }
    }
}

pub(crate) fn help_text() -> String {
    "Teleport hands this workspace to a cloud VM (E2B).\n\
     - `/teleport [template]` — pack the workspace (tracked + untracked, ignores honored) and upload to a fresh sandbox\n\
     - `/teleport pull <sandbox-id>` — fetch the sandbox workspace back into .supercharge/teleport/\n\
     - `/teleport kill <sandbox-id>` — stop the sandbox\n\
     Requires E2B_API_KEY in the environment."
        .to_string()
}

/// Run `program args` in `dir`, optionally feeding stdin, capturing stdout.
/// Fails with the command's stderr when the exit status is unsuccessful or
/// when a NUL-separated file list is required but `git ls-files` returned nothing.
pub(crate) async fn run_local(
    program: &str,
    args: &[&str],
    dir: &Path,
    stdin: Option<Vec<u8>>,
) -> Result<Vec<u8>, String> {
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(dir)
        .stdin(if stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{program} failed to start: {e}"))?;
    if let (Some(input), Some(mut stdin_handle)) = (stdin, child.stdin.take()) {
        stdin_handle
            .write_all(&input)
            .await
            .map_err(|e| format!("{program} stdin failed: {e}"))?;
        stdin_handle
            .shutdown()
            .await
            .map_err(|e| format!("{program} stdin close failed: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("{program} failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{program} {} exited with {}: {}",
            args.join(" "),
            output.status,
            stderr.trim()
        ));
    }
    Ok(output.stdout)
}

/// Pack the workspace: every tracked + untracked non-ignored file, current contents.
/// Returns the tarball bytes. Fails when the cwd is not a git work tree or the pack
/// exceeds [`MAX_PACK_BYTES`].
pub(crate) async fn pack_workspace(cwd: &Path, work_dir: &Path) -> Result<Vec<u8>, String> {
    let list = run_local(
        "git",
        &["ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        cwd,
        None,
    )
    .await?;
    if list.iter().all(|&b| b == 0) {
        return Err("not a git work tree (git ls-files returned nothing)".to_string());
    }
    let pack_path = work_dir.join("supercharge-teleport.tgz");
    let pack_path_str = pack_path.to_string_lossy().to_string();
    run_local(
        "tar",
        &[
            "--null",
            "-czf",
            &pack_path_str,
            "-C",
            &cwd.to_string_lossy(),
            "--files-from",
            "-",
        ],
        work_dir,
        Some(list),
    )
    .await?;
    let bytes = tokio::fs::read(&pack_path)
        .await
        .map_err(|e| format!("read pack failed: {e}"))?;
    let _ = tokio::fs::remove_file(&pack_path).await;
    if bytes.len() as u64 > MAX_PACK_BYTES {
        return Err(format!(
            "workspace pack is {} MiB, over the {} MiB teleport cap; add heavy paths to .gitignore",
            bytes.len() / (1024 * 1024),
            MAX_PACK_BYTES / (1024 * 1024)
        ));
    }
    Ok(bytes)
}

pub(crate) struct TeleportExecutor {
    inner: supercharge_superagent::E2bExecutor,
}

impl TeleportExecutor {
    pub(crate) fn new() -> Result<Self, String> {
        let Some(api_key) = std::env::var("E2B_API_KEY").ok() else {
            return Err("E2B_API_KEY is not set; export it and retry".to_string());
        };
        Ok(Self {
            inner: supercharge_superagent::E2bExecutor::new(
                supercharge_superagent::SandboxPlan {
                    timeout_seconds: 3600,
                    ..Default::default()
                },
                Some(api_key),
            ),
        })
    }

    /// Create a sandbox, upload and extract the workspace pack, and report a summary.
    pub(crate) async fn push(
        &self,
        cwd: &Path,
        work_dir: &Path,
        template: Option<&str>,
    ) -> Result<String, String> {
        let bytes = pack_workspace(cwd, work_dir).await?;
        let executor = match template {
            Some(template) => {
                let Some(api_key) = std::env::var("E2B_API_KEY").ok() else {
                    return Err("E2B_API_KEY is not set; export it and retry".to_string());
                };
                supercharge_superagent::E2bExecutor::new(
                    supercharge_superagent::SandboxPlan {
                        template: template.to_string(),
                        timeout_seconds: 3600,
                        ..Default::default()
                    },
                    Some(api_key),
                )
            }
            None => supercharge_superagent::E2bExecutor::new(
                supercharge_superagent::SandboxPlan {
                    timeout_seconds: 3600,
                    ..Default::default()
                },
                std::env::var("E2B_API_KEY").ok(),
            ),
        };
        let workspace_root = cwd.to_string_lossy().to_string();
        let status = executor
            .ensure_session(None, workspace_root)
            .await
            .map_err(|e| format!("sandbox create failed: {e}"))?;
        let sandbox_id = status.sandbox_id.clone();
        executor
            .upload_file(&sandbox_id, "/home/user/teleport-src.tgz", bytes.to_vec())
            .await
            .map_err(|e| format!("upload failed: {e}"))?;
        executor
            .execute_persistent(supercharge_superagent::PersistentExecutionRequest {
                sandbox_id: Some(sandbox_id.clone()),
                command: "mkdir -p /home/user/teleport && tar xzf /home/user/teleport-src.tgz -C /home/user/teleport".to_string(),
                cwd: Some("/home/user".to_string()),
                workspace_root: "/home/user".to_string(),
                timeout_seconds: Some(300),
            })
            .await
            .map_err(|e| format!("extract failed: {e}"))?;

        Ok(format!(
            "Teleported to sandbox {sandbox_id}.\nWorkspace: {SANDBOX_DIR} (template {}, {} CPU, {} MiB RAM).\nBring it back with: /teleport pull {sandbox_id}\nStop it with: /teleport kill {sandbox_id}",
            status.template_id, status.cpu_count, status.memory_mb,
        ))
    }

    /// Tar the sandbox workspace and store the bundle locally.
    pub(crate) async fn pull(&self, cwd: &Path, sandbox_id: &str) -> Result<String, String> {
        self.inner
            .execute_persistent(supercharge_superagent::PersistentExecutionRequest {
                sandbox_id: Some(sandbox_id.to_string()),
                command: "rm -f /home/user/teleport-out.tgz && tar czf /home/user/teleport-out.tgz -C /home/user/teleport .".to_string(),
                cwd: Some("/home/user".to_string()),
                workspace_root: "/home/user".to_string(),
                timeout_seconds: Some(300),
            })
            .await
            .map_err(|e| format!("sandbox pack failed: {e}"))?;
        let bytes = self
            .inner
            .download_file(sandbox_id, "/home/user/teleport-out.tgz")
            .await
            .map_err(|e| format!("download failed: {e}"))?;
        let out_dir = cwd.join(".supercharge").join("teleport");
        tokio::fs::create_dir_all(&out_dir)
            .await
            .map_err(|e| format!("create {} failed: {e}", out_dir.display()))?;
        let out_path = out_dir.join(format!("{sandbox_id}.tgz"));
        tokio::fs::write(&out_path, &bytes)
            .await
            .map_err(|e| format!("write {} failed: {e}", out_path.display()))?;
        let unpack_dir = format!(
            "../{}-pulled",
            out_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        );
        Ok(format!(
            "Pulled sandbox {sandbox_id} ({} KiB) to {}.\nInspect with: tar tzf {}\nUnpack with: mkdir -p {unpack_dir} && tar xzf {} -C {unpack_dir}",
            bytes.len() / 1024,
            out_path.display(),
            out_path.display(),
            out_path.display(),
        ))
    }

    pub(crate) async fn kill(&self, sandbox_id: &str) -> Result<String, String> {
        let killed = self
            .inner
            .kill(sandbox_id)
            .await
            .map_err(|e| format!("kill failed: {e}"))?;
        if killed {
            Ok(format!("Sandbox {sandbox_id} stopped."))
        } else {
            Ok(format!(
                "Sandbox {sandbox_id} reported no kill — it is likely already stopped."
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_routes_subcommands() {
        assert_eq!(
            TeleportOp::parse(""),
            TeleportOp::Push { template: None }
        );
        assert_eq!(
            TeleportOp::parse("rust-build-16"),
            TeleportOp::Push {
                template: Some("rust-build-16".to_string())
            }
        );
        assert_eq!(
            TeleportOp::parse("pull sb_123"),
            TeleportOp::Pull {
                sandbox_id: "sb_123".to_string()
            }
        );
        assert_eq!(
            TeleportOp::parse("kill sb_123"),
            TeleportOp::Kill {
                sandbox_id: "sb_123".to_string()
            }
        );
        assert_eq!(TeleportOp::parse("pull"), TeleportOp::Help);
        assert_eq!(TeleportOp::parse("wat x y"), TeleportOp::Help);
    }

    #[test]
    fn pack_reports_missing_git_work_tree() {
        let dir = std::env::temp_dir().join(format!("sc-teleport-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt.block_on(pack_workspace(&dir, &dir)).unwrap_err();
        assert!(err.contains("git"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
