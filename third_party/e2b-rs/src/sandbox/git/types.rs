//! Public types for sandbox git repository status and branch operations.

/// Scope for a `git config` operation.
///
/// Maps to the `--global`, `--local`, and `--system` flags passed to `git config`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitConfigScope {
    /// Apply the setting globally (`~/.gitconfig`).
    Global,
    /// Apply the setting to the current repository (`.git/config`).
    Local,
    /// Apply the setting system-wide (`/etc/gitconfig`).
    System,
}

/// Normalized status label derived from the git porcelain index/worktree codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitStatusLabel {
    /// The file has a merge conflict (at least one status code is `U`).
    Conflict,
    /// The file was renamed (`R` code).
    Renamed,
    /// The file was copied (`C` code).
    Copied,
    /// The file was deleted (`D` code).
    Deleted,
    /// The file was added (`A` code).
    Added,
    /// The file was modified (`M` code).
    Modified,
    /// The file's type changed (e.g., regular file ↔ symlink; `T` code).
    TypeChange,
    /// The file is untracked (`??`).
    Untracked,
    /// The file has an unrecognised status code.
    Unknown,
}

/// Parsed git status entry for a single file.
#[derive(Debug, Clone)]
pub struct GitFileStatus {
    /// Path relative to the repository root.
    pub name: String,
    /// Normalized status label.
    pub status: GitStatusLabel,
    /// Index (staging area) status character from porcelain output.
    pub index_status: char,
    /// Working-tree status character from porcelain output.
    pub working_tree_status: char,
    /// Whether the change is currently staged (in the index).
    pub staged: bool,
    /// Original path when the file was renamed or copied.
    pub renamed_from: Option<String>,
}

/// Parsed output of `git status --porcelain=1 -b`.
#[derive(Debug, Clone)]
pub struct GitStatus {
    /// Current branch name, if the HEAD is attached to a branch.
    pub current_branch: Option<String>,
    /// Upstream tracking branch name, if configured.
    pub upstream: Option<String>,
    /// Number of commits the branch is ahead of upstream.
    pub ahead: i32,
    /// Number of commits the branch is behind upstream.
    pub behind: i32,
    /// Whether HEAD is in a detached state.
    pub detached: bool,
    /// Per-file status entries.
    pub file_status: Vec<GitFileStatus>,
    /// `true` when there are no tracked or untracked changes.
    pub is_clean: bool,
    /// `true` when there is at least one tracked or untracked change.
    pub has_changes: bool,
    /// `true` when there is at least one staged change.
    pub has_staged: bool,
    /// `true` when there is at least one untracked file.
    pub has_untracked: bool,
    /// `true` when there is at least one merge conflict.
    pub has_conflicts: bool,
    /// Total number of changed files (staged + unstaged + untracked).
    pub total_count: usize,
    /// Number of files with staged changes.
    pub staged_count: usize,
    /// Number of files with unstaged changes (total minus staged).
    pub unstaged_count: usize,
    /// Number of untracked files.
    pub untracked_count: usize,
    /// Number of files with merge conflicts.
    pub conflict_count: usize,
}

/// Parsed output of `git branch --format=%(refname:short)\t%(HEAD)`.
#[derive(Debug, Clone)]
pub struct GitBranches {
    /// All local branch names.
    pub branches: Vec<String>,
    /// The currently checked-out branch, if any.
    pub current_branch: Option<String>,
}
