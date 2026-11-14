//! Git helpers for the validation gate. All functions return errors instead of panicking.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::no_mistake::pipeline::GateError;

#[derive(Debug, Clone)]
pub struct RepoInfo {
    pub root: PathBuf,
    pub branch: String,
    pub head: String,
    pub default_branch: String,
}

pub fn inspect_repo(cwd: &Path, preferred_base: Option<&str>) -> Result<RepoInfo, GateError> {
    let root = git_stdout(cwd, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(root);
    let branch = git_stdout(&root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch == "HEAD" {
        return Err(GateError::user(
            "The repository is in a detached HEAD state. Check out a branch before running /no-mistake.",
        ));
    }
    let head = git_stdout(&root, &["rev-parse", "HEAD"])?;
    let default_branch = resolve_base_branch(&root, preferred_base);
    Ok(RepoInfo {
        root,
        branch,
        head,
        default_branch,
    })
}

pub fn resolve_base_branch(root: &Path, preferred: Option<&str>) -> String {
    if let Some(name) = preferred.map(str::trim).filter(|s| !s.is_empty()) {
        return name.to_string();
    }
    if let Ok(symbolic) = git_stdout(root, &["symbolic-ref", "refs/remotes/origin/HEAD"])
        && let Some(name) = symbolic.rsplit('/').next()
        && !name.is_empty()
    {
        return name.to_string();
    }
    for candidate in ["main", "master"] {
        if git_ok(root, &["rev-parse", "--verify", "--quiet", candidate])
            || git_ok(
                root,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("origin/{candidate}"),
                ],
            )
        {
            return candidate.to_string();
        }
    }
    "main".to_string()
}

pub fn is_default_branch(branch: &str, default_branch: &str) -> bool {
    branch == default_branch
}

pub fn has_uncommitted_changes(root: &Path) -> Result<bool, GateError> {
    // A clean tree produces empty porcelain output; that is success, not an error.
    let status = git_maybe(root, &["status", "--porcelain"])?;
    Ok(!status.is_empty())
}

pub fn merge_base(root: &Path, base: &str, head: &str) -> Result<String, GateError> {
    let base_ref = first_existing_ref(root, &[format!("origin/{base}"), base.to_string()])
        .unwrap_or_else(|| base.to_string());
    git_stdout(root, &["merge-base", &base_ref, head])
}

pub fn commits_ahead(root: &Path, base: &str, head: &str) -> Result<Vec<String>, GateError> {
    let merge_base = merge_base(root, base, head)?;
    let log = git_maybe(
        root,
        &["log", "--format=%s", &format!("{merge_base}..{head}")],
    )?;
    Ok(log
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub fn unified_diff(root: &Path, base: &str) -> Result<String, GateError> {
    let base_ref = first_existing_ref(root, &[format!("origin/{base}"), base.to_string()])
        .unwrap_or_else(|| base.to_string());
    let merge = git_maybe(root, &["merge-base", &base_ref, "HEAD"])?;
    let from = if merge.is_empty() { base_ref } else { merge };
    git_maybe(root, &["diff", "--no-color", &from, "HEAD"])
}

pub fn first_existing_ref(root: &Path, refs: &[String]) -> Option<String> {
    for r in refs {
        if git_ok(root, &["rev-parse", "--verify", "--quiet", r]) {
            return Some(r.clone());
        }
    }
    None
}

/// Create a disposable worktree on a new gate branch pointing at the current HEAD.
pub fn create_isolated_worktree(
    root: &Path,
    home: &Path,
    branch: &str,
) -> Result<(PathBuf, String, String), GateError> {
    let stamp = unique_stamp();
    let repo_id = repo_id(root);
    let path = home
        .join("no-mistake")
        .join("worktrees")
        .join(&repo_id)
        .join(&stamp);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| GateError::io("create the worktree directory", e))?;
    }
    if path.exists() {
        return Err(GateError::user(format!(
            "Worktree path {} already exists.",
            path.display()
        )));
    }
    let gate_branch = unique_gate_branch(root, branch, &stamp);
    let path_str = path.to_string_lossy().to_string();
    git_run(
        root,
        &["worktree", "add", "-b", &gate_branch, &path_str, "HEAD"],
    )?;
    Ok((path, gate_branch, stamp))
}

pub fn remove_worktree(root: &Path, worktree: &Path, gate_branch: &str) {
    let path = worktree.to_string_lossy().to_string();
    let _ = git_run(root, &["worktree", "remove", "--force", &path]);
    let _ = git_run(root, &["branch", "-D", gate_branch]);
}

pub fn rebase_onto(worktree: &Path, base: &str) -> Result<(), GateError> {
    let _ = git_run(worktree, &["fetch", "origin", base]);
    let onto = first_existing_ref(
        worktree,
        &[format!("origin/{base}"), base.to_string()],
    )
    .ok_or_else(|| {
        GateError::user(format!(
            "Could not find base branch '{base}' locally or on origin. Pass --base-branch or set [no_mistake].base_branch."
        ))
    })?;
    match git_run(worktree, &["rebase", &onto]) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = git_run(worktree, &["rebase", "--abort"]);
            Err(GateError::user(format!(
                "Rebase onto {onto} failed. Resolve the conflict in a regular worktree and run /no-mistake again. {err}"
            )))
        }
    }
}

pub fn commit_all(worktree: &Path, message: &str) -> Result<bool, GateError> {
    let _ = git_run(worktree, &["add", "-A"]);
    if !has_uncommitted_changes(worktree)? {
        return Ok(false);
    }
    git_run(worktree, &["commit", "-m", message])?;
    Ok(true)
}

pub fn run_shell_command(worktree: &Path, command: &str) -> Result<CommandResult, GateError> {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    };
    cmd.current_dir(worktree);
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    let output = cmd
        .output()
        .map_err(|e| GateError::io(format!("run `{command}`"), e))?;
    Ok(CommandResult::from_output(output))
}

#[derive(Debug, Clone)]
pub struct CommandResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
}

impl CommandResult {
    fn from_output(output: Output) -> Self {
        Self {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            code: output.status.code(),
        }
    }

    pub fn combined(&self) -> String {
        let mut s = self.stdout.clone();
        if !self.stderr.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&self.stderr);
        }
        s
    }
}

pub fn infer_test_command(root: &Path) -> Option<String> {
    if root.join("Cargo.toml").is_file() {
        return Some("cargo test".to_string());
    }
    if root.join("go.mod").is_file() {
        return Some("go test ./...".to_string());
    }
    if root.join("pytest.ini").is_file()
        || root.join("pyproject.toml").is_file()
        || root.join("tox.ini").is_file()
    {
        return Some("pytest".to_string());
    }
    if root.join("package.json").is_file() {
        return Some("npm test".to_string());
    }
    None
}

pub fn infer_lint_command(root: &Path) -> Option<String> {
    if root.join("Cargo.toml").is_file() {
        return Some("cargo clippy --all-targets".to_string());
    }
    if root.join("go.mod").is_file() {
        return Some("go vet ./...".to_string());
    }
    if root.join("package.json").is_file() {
        return Some("npm run lint".to_string());
    }
    None
}

fn unique_gate_branch(root: &Path, branch: &str, stamp: &str) -> String {
    let sanitized = sanitize_ref(branch);
    let mut name = format!("supercharge/no-mistake/{sanitized}/{stamp}");
    let mut n = 1u32;
    while git_ok(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    ) {
        n = n.saturating_add(1);
        name = format!("supercharge/no-mistake/{sanitized}/{stamp}-{n}");
        if n > 32 {
            break;
        }
    }
    name
}

fn sanitize_ref(branch: &str) -> String {
    let mut out = String::new();
    for ch in branch.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    if out.is_empty() {
        out.push_str("branch");
    }
    out
}

fn repo_id(root: &Path) -> String {
    let text = root.to_string_lossy();
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:012x}")
}

fn unique_stamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let random = std::process::id();
    format!("{now:x}-{random:x}")
}

pub fn git_stdout(cwd: &Path, args: &[&str]) -> Result<String, GateError> {
    let text = git_maybe(cwd, args)?;
    if text.is_empty() {
        return Err(GateError::user(format!(
            "git {} returned no output.",
            args.join(" ")
        )));
    }
    Ok(text)
}

pub fn git_maybe(cwd: &Path, args: &[&str]) -> Result<String, GateError> {
    let output = git_output(cwd, args)?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let out = String::from_utf8_lossy(&output.stdout);
        let detail = if err.trim().is_empty() {
            out.trim().to_string()
        } else {
            err.trim().to_string()
        };
        return Err(GateError::user(format!(
            "git {} failed: {detail}",
            args.join(" ")
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn git_run(cwd: &Path, args: &[&str]) -> Result<(), GateError> {
    git_maybe(cwd, args).map(|_| ())
}

pub fn git_ok(cwd: &Path, args: &[&str]) -> bool {
    git_output(cwd, args)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<Output, GateError> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    cmd.current_dir(cwd);
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    cmd.output()
        .map_err(|e| GateError::io(format!("run git {}", args.join(" ")), e))
}

#[allow(dead_code)]
pub fn os_str_list(args: &[impl AsRef<OsStr>]) -> String {
    args.iter()
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}
