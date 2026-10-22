//! Curated per-project note: `memory-v2/projects/<workspace id>/PROJECT.md`.
//!
//! One short, hand-curated summary of the project (purpose, layout, how to
//! build and test, conventions), capped at [`PROJECT_NOTE_MAX_CHARS`]. It is
//! shown in full at session start, unlike topics which are listed by name.

use std::path::{Path, PathBuf};

pub const PROJECT_NOTE_FILE: &str = "PROJECT.md";
pub const PROJECT_NOTE_MAX_CHARS: usize = 1500;

/// Project directory for a v2 workspace scope directory
/// (`<root>/workspaces/<id>` maps to `<root>/projects/<id>`).
pub fn project_dir_for_workspace(workspace_dir: &Path) -> Option<PathBuf> {
    let id = workspace_dir.file_name()?;
    let workspaces = workspace_dir.parent()?;
    if workspaces.file_name()? != "workspaces" {
        return None;
    }
    Some(workspaces.parent()?.join("projects").join(id))
}

/// Create the project directory with owner-only permissions.
pub fn ensure_project_dir(storage_root: &Path, project_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(project_dir)?;
    crate::secret_guard::restrict_dir_chain(storage_root, project_dir);
    Ok(())
}

/// Read the project note, if present, bounded to a few times the cap.
pub fn read_project_note(project_dir: &Path) -> Option<String> {
    let path = project_dir.join(PROJECT_NOTE_FILE);
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > (PROJECT_NOTE_MAX_CHARS * 8) as u64 {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.chars().take(PROJECT_NOTE_MAX_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_workspace_to_project_dir() {
        assert_eq!(
            project_dir_for_workspace(Path::new("/h/memory-v2/workspaces/ws-1")),
            Some(PathBuf::from("/h/memory-v2/projects/ws-1"))
        );
        assert_eq!(project_dir_for_workspace(Path::new("/h/memory-v2/global")), None);
    }

    #[test]
    fn reads_trimmed_and_capped_note() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(read_project_note(temp.path()), None);
        std::fs::write(temp.path().join(PROJECT_NOTE_FILE), format!("  {}  ", "x".repeat(2000))).unwrap();
        assert_eq!(read_project_note(temp.path()).unwrap().len(), PROJECT_NOTE_MAX_CHARS);
    }
}
