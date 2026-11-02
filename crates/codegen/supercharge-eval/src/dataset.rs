//! Task dataset loading. A dataset is a JSONL file (or a directory of them)
//! with one task per line.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::EvalError;

fn default_timeout_seconds() -> u64 {
    300
}

/// One eval task: instructions for the agent plus a grading contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub task_id: String,
    pub instructions: String,
    /// Local repository the attempt worktrees are created from. Cloning from a
    /// remote URL is out of scope for v1.
    pub repo_path: PathBuf,
    /// Commit to check out; defaults to the repository HEAD.
    pub base_commit: Option<String>,
    /// Shell command run in a clean checkout with the patch applied; a zero
    /// exit status grades the attempt as a pass. Tasks without a `test_cmd`
    /// grade on patch application alone.
    pub test_cmd: Option<String>,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    /// Extra environment variables for the agent and the test command.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Load tasks from a JSONL file or a directory of `*.jsonl` files. Blank lines
/// and `#` comments are ignored; duplicate `task_id`s are rejected.
pub fn load_dataset(path: &Path) -> Result<Vec<Task>, EvalError> {
    let mut files = Vec::new();
    if path.is_dir() {
        let mut entries = std::fs::read_dir(path)?
            .map(|entry| entry.map(|e| e.path()))
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort();
        for entry in entries {
            if entry.extension().is_some_and(|ext| ext == "jsonl") {
                files.push(entry);
            }
        }
        if files.is_empty() {
            return Err(EvalError::Dataset(format!(
                "no .jsonl files found in {}",
                path.display()
            )));
        }
    } else {
        files.push(path.to_path_buf());
    }

    let mut tasks = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for file in &files {
        let text = std::fs::read_to_string(file)?;
        for (line_no, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let task: Task = serde_json::from_str(line).map_err(|e| {
                EvalError::Dataset(format!("{}:{}: {e}", file.display(), line_no + 1))
            })?;
            if !seen.insert(task.task_id.clone()) {
                return Err(EvalError::Dataset(format!(
                    "duplicate task_id `{}` in dataset {}",
                    task.task_id,
                    path.display()
                )));
            }
            tasks.push(task);
        }
    }
    if tasks.is_empty() {
        return Err(EvalError::Dataset(format!(
            "dataset {} contains no tasks",
            path.display()
        )));
    }
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use super::load_dataset;
    use std::path::Path;

    #[test]
    fn parses_jsonl_and_rejects_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tasks.jsonl");
        std::fs::write(
            &file,
            concat!(
                "# comment\n",
                r#"{"task_id":"a","instructions":"fix it","repo_path":"/tmp/repo","test_cmd":"true"}"#,
                "\n\n",
                r#"{"task_id":"b","instructions":"again","repo_path":"/tmp/repo","base_commit":"abc123"}"#,
                "\n",
            ),
        )
        .unwrap();
        let tasks = load_dataset(&file).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].task_id, "a");
        assert_eq!(tasks[0].timeout_seconds, 300);
        assert_eq!(tasks[1].base_commit.as_deref(), Some("abc123"));
        assert!(tasks[1].test_cmd.is_none());

        let dup = dir.path().join("dup.jsonl");
        std::fs::write(
            &dup,
            r#"{"task_id":"a","instructions":"x","repo_path":"/tmp/repo"}"#,
        )
        .unwrap();
        let err = load_dataset(&dir.path()).unwrap_err().to_string();
        assert!(err.contains("duplicate task_id"), "{err}");
    }

    #[test]
    fn missing_dataset_is_an_error() {
        let err = load_dataset(Path::new("/nonexistent/tasks.jsonl")).unwrap_err();
        assert!(err.to_string().contains("dataset error") || err.to_string().contains("io error"));
    }
}
