//! Built-in eval presets: self-contained task sets that materialize temporary
//! git repositories so scores are comparable across Supercharge versions.

use std::path::Path;
use std::process::Command;

use crate::dataset::Task;
use crate::EvalError;

/// Named built-in task set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// Tiny text-file fix: write `fixed` into `value.txt`.
    Smoke,
    /// Mini Rust crate with a failing unit test the agent must make pass.
    Rust,
    /// Static web page whose visible text must match a grading contract.
    Web,
}

impl Preset {
    pub fn parse(name: &str) -> Result<Self, EvalError> {
        match name.trim().to_ascii_lowercase().as_str() {
            "smoke" => Ok(Self::Smoke),
            "rust" => Ok(Self::Rust),
            "web" => Ok(Self::Web),
            other => Err(EvalError::Dataset(format!(
                "unknown preset `{other}`; expected smoke, rust, or web"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Smoke => "smoke",
            Self::Rust => "rust",
            Self::Web => "web",
        }
    }

    /// All presets, in documentation order.
    pub fn all() -> &'static [Self] {
        &[Self::Smoke, Self::Rust, Self::Web]
    }
}

/// Materialize the preset under `parent` (creates `parent/<preset>/…` git
/// repos) and return the tasks whose `repo_path`s point into that tree.
/// `parent` should outlive the eval run (for example the run's out_dir).
pub fn materialize(preset: Preset, parent: &Path) -> Result<Vec<Task>, EvalError> {
    let root = parent.join("presets").join(preset.as_str());
    std::fs::create_dir_all(&root)?;
    match preset {
        Preset::Smoke => Ok(vec![smoke_task(&root)?]),
        Preset::Rust => Ok(vec![rust_task(&root)?]),
        Preset::Web => Ok(vec![web_task(&root)?]),
    }
}

fn git_init_commit(repo: &Path, message: &str) -> Result<(), EvalError> {
    run_git(repo, &["init", "-q"])?;
    run_git(repo, &["config", "user.email", "eval-preset@example.com"])?;
    run_git(repo, &["config", "user.name", "eval-preset"])?;
    run_git(repo, &["add", "-A"])?;
    // Normalize the initial branch name so worktree checkouts are predictable.
    let _ = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["branch", "-M", "main"])
        .output();
    run_git(repo, &["commit", "-q", "-m", message])?;
    Ok(())
}

fn run_git(repo: &Path, args: &[&str]) -> Result<(), EvalError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()?;
    if out.status.success() {
        return Ok(());
    }
    // `checkout -b main` fails when main already exists; callers that care
    // handle that. For everything else surface stderr.
    Err(EvalError::Git {
        args: args.join(" "),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    })
}

fn smoke_task(root: &Path) -> Result<Task, EvalError> {
    let repo = root.join("fix-value");
    if !repo.join(".git").exists() {
        std::fs::create_dir_all(&repo)?;
        std::fs::write(repo.join("value.txt"), "wrong\n")?;
        std::fs::write(
            repo.join("README.md"),
            "Smoke preset: make value.txt contain exactly `fixed`.\n",
        )?;
        git_init_commit(&repo, "init smoke")?;
    }
    Ok(Task {
        task_id: "smoke-fix-value".into(),
        instructions: "Make value.txt contain exactly the text `fixed` (no trailing newline required beyond a single line). Do not change other files.".into(),
        repo_path: repo,
        base_commit: None,
        test_cmd: Some(r#"test "$(cat value.txt)" = "fixed""#.into()),
        timeout_seconds: 120,
        env: Default::default(),
    })
}

fn rust_task(root: &Path) -> Result<Task, EvalError> {
    let repo = root.join("add-one");
    if !repo.join(".git").exists() {
        std::fs::create_dir_all(repo.join("src"))?;
        std::fs::write(
            repo.join("Cargo.toml"),
            r#"[package]
name = "add_one"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        std::fs::write(
            repo.join("src/lib.rs"),
            r#"/// Return n + 1. The unit test below currently fails; make it pass.
pub fn add_one(n: i32) -> i32 {
    n
}

#[cfg(test)]
mod tests {
    use super::add_one;

    #[test]
    fn increments() {
        assert_eq!(add_one(1), 2);
        assert_eq!(add_one(-1), 0);
    }
}
"#,
        )?;
        git_init_commit(&repo, "init rust")?;
    }
    Ok(Task {
        task_id: "rust-add-one".into(),
        instructions: "In this Rust crate, make `add_one` return n + 1 so `cargo test` passes. Keep the public API and tests unchanged.".into(),
        repo_path: repo,
        base_commit: None,
        test_cmd: Some("cargo test -q".into()),
        timeout_seconds: 300,
        env: Default::default(),
    })
}

fn web_task(root: &Path) -> Result<Task, EvalError> {
    let repo = root.join("greeting");
    if !repo.join(".git").exists() {
        std::fs::create_dir_all(&repo)?;
        std::fs::write(
            repo.join("index.html"),
            r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>Greeting</title></head>
<body>
  <h1 id="greeting">Hello, stranger</h1>
</body>
</html>
"#,
        )?;
        std::fs::write(
            repo.join("README.md"),
            "Web preset: change the greeting heading text to exactly `Hello, Supercharge`.\n",
        )?;
        git_init_commit(&repo, "init web")?;
    }
    Ok(Task {
        task_id: "web-greeting".into(),
        instructions: "Edit index.html so the element with id `greeting` contains exactly the text `Hello, Supercharge` (no extra markup inside the heading).".into(),
        repo_path: repo,
        base_commit: None,
        test_cmd: Some(
            r#"grep -q 'id="greeting">Hello, Supercharge</h1>' index.html"#.into(),
        ),
        timeout_seconds: 120,
        env: Default::default(),
    })
}

/// Resolve a CLI `--preset` name into tasks under `parent`.
pub fn load_preset(name: &str, parent: &Path) -> Result<Vec<Task>, EvalError> {
    materialize(Preset::parse(name)?, parent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_known_names() {
        assert_eq!(Preset::parse("smoke").unwrap(), Preset::Smoke);
        assert_eq!(Preset::parse("RUST").unwrap(), Preset::Rust);
        assert!(Preset::parse("nope").is_err());
    }

    #[test]
    fn smoke_materializes_gradable_repo() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = materialize(Preset::Smoke, dir.path()).unwrap();
        assert_eq!(tasks.len(), 1);
        assert!(tasks[0].repo_path.join("value.txt").exists());
        assert!(tasks[0].repo_path.join(".git").exists());
        // Re-materializing is idempotent.
        let again = materialize(Preset::Smoke, dir.path()).unwrap();
        assert_eq!(again[0].task_id, tasks[0].task_id);
    }

    #[test]
    fn rust_and_web_materialize() {
        let dir = tempfile::tempdir().unwrap();
        let rust = materialize(Preset::Rust, dir.path()).unwrap();
        assert!(rust[0].repo_path.join("Cargo.toml").exists());
        let web = materialize(Preset::Web, dir.path()).unwrap();
        assert!(web[0].repo_path.join("index.html").exists());
    }

    #[test]
    fn smoke_test_cmd_fails_before_fix() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = materialize(Preset::Smoke, dir.path()).unwrap();
        let status = Command::new("sh")
            .arg("-c")
            .arg(tasks[0].test_cmd.as_deref().unwrap())
            .current_dir(&tasks[0].repo_path)
            .status()
            .unwrap();
        assert!(!status.success());
        std::fs::write(tasks[0].repo_path.join("value.txt"), "fixed").unwrap();
        let status = Command::new("sh")
            .arg("-c")
            .arg(tasks[0].test_cmd.as_deref().unwrap())
            .current_dir(&tasks[0].repo_path)
            .status()
            .unwrap();
        assert!(status.success());
    }
}
