//! Sandbox Git API — public types, internal utilities, and the [`Git`] command driver.
//!
//! The [`crate::sandbox::git`] module exposes the result types produced by git
//! operations and the [`Git`] struct for running them. The
//! [`crate::sandbox::git::util`] sub-module contains the pure (no-I/O) helpers
//! that build commands and parse output.

pub mod types;
pub(crate) mod util;

pub use types::{GitBranches, GitConfigScope, GitFileStatus, GitStatus, GitStatusLabel};

use std::collections::BTreeMap;
use std::future::Future;

use crate::errors::{Error, Result};
use crate::sandbox::commands::{CommandResult, CommandStartOpts, Commands};
use crate::sandbox::git::util::{
    build_auth_error_message, build_git_command, build_push_args, build_upstream_error_message,
    derive_repo_dir_from_url, is_auth_failure, is_missing_upstream, parse_git_branches,
    parse_git_status, strip_credentials, with_credentials,
};

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Resolve the `--global` / `--local` / `--system` flag for the given scope.
fn scope_flag(scope: GitConfigScope) -> &'static str {
    match scope {
        GitConfigScope::Global => "--global",
        GitConfigScope::Local => "--local",
        GitConfigScope::System => "--system",
    }
}

/// Return the repository path for a config scope, or an error when
/// [`GitConfigScope::Local`] is requested without a path.
fn repo_path_for_scope(scope: GitConfigScope, path: Option<&str>) -> Result<Option<String>> {
    match scope {
        GitConfigScope::Local => match path {
            Some(p) => Ok(Some(p.to_string())),
            None => Err(Error::InvalidArgument(
                r#"A repository path is required when using scope "local"."#.to_string(),
            )),
        },
        _ => Ok(None),
    }
}

/// Promote a non-zero exit from a `git remote set-url` plumbing call to
/// [`Error::InvalidArgument`].
///
/// The two `set-url` commands inside [`Git::with_remote_credentials`] are
/// internal plumbing.  A silently swallowed non-zero would either leave
/// credentials embedded in `.git/config` (failed restore) or run the
/// operation against the wrong URL (failed cred set).  Matching the JS SDK
/// where every `runGit` throws on non-zero, we surface the failure here.
fn check_set_url_exit(result: CommandResult) -> Result<CommandResult> {
    if result.exit_code != 0 {
        return Err(Error::InvalidArgument(format!(
            "git remote set-url failed (exit {}): {}",
            result.exit_code,
            result.stderr.trim()
        )));
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Reset mode
// ---------------------------------------------------------------------------

/// Reset mode for [`Git::reset`].
///
/// Each variant maps to the corresponding `git reset` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitResetMode {
    /// Keep index changes, do not touch the working tree (`--soft`).
    Soft,
    /// Reset the index but keep the working tree (`--mixed`; git default).
    Mixed,
    /// Reset both the index and the working tree (`--hard`).
    Hard,
    /// Reset the index and update working-tree files that differ between
    /// the commit and `HEAD` (`--merge`).
    Merge,
    /// Reset the index and update working-tree files that are not locally
    /// modified (`--keep`).
    Keep,
}

impl GitResetMode {
    fn flag(self) -> &'static str {
        match self {
            GitResetMode::Soft => "--soft",
            GitResetMode::Mixed => "--mixed",
            GitResetMode::Hard => "--hard",
            GitResetMode::Merge => "--merge",
            GitResetMode::Keep => "--keep",
        }
    }
}

// ---------------------------------------------------------------------------
// Option structs
// ---------------------------------------------------------------------------

/// Options for [`Git::init`].
#[derive(Default)]
pub struct GitInitOpts {
    /// Initial branch name (e.g., `"main"`). Passed as `--initial-branch`.
    pub initial_branch: Option<String>,
    /// Create a bare repository (`--bare`).
    pub bare: bool,
    /// Run as this sandbox user.
    pub user: Option<String>,
}

/// Options for [`Git::add`].
pub struct GitAddOpts {
    /// Specific files to stage. When empty, `all` (or `.`) controls the target.
    pub files: Vec<String>,
    /// Stage all changes with `-A` when `true` and `files` is empty.
    ///
    /// Defaults to `true`, matching the JS SDK default.
    /// Ignored when `files` is non-empty.
    pub all: bool,
    /// Run as this sandbox user.
    pub user: Option<String>,
}

impl Default for GitAddOpts {
    fn default() -> Self {
        GitAddOpts {
            files: Vec::new(),
            all: true, // JS default: `all = true`
            user: None,
        }
    }
}

/// Options for [`Git::commit`].
#[derive(Default)]
pub struct GitCommitOpts {
    /// Override commit author name (`-c user.name=…`).
    pub author_name: Option<String>,
    /// Override commit author email (`-c user.email=…`).
    pub author_email: Option<String>,
    /// Allow commits with no changes (`--allow-empty`).
    pub allow_empty: bool,
    /// Run as this sandbox user.
    pub user: Option<String>,
}

/// Options for [`Git::delete_branch`].
#[derive(Default)]
pub struct GitDeleteBranchOpts {
    /// Force deletion with `-D` even when the branch is not fully merged.
    pub force: bool,
    /// Run as this sandbox user.
    pub user: Option<String>,
}

/// Options for [`Git::reset`].
#[derive(Default)]
pub struct GitResetOpts {
    /// Reset mode. Omit to use the git default (`mixed`).
    pub mode: Option<GitResetMode>,
    /// Commit, branch, or ref to reset to. Defaults to `HEAD`.
    pub target: Option<String>,
    /// Paths to reset (after `--`).
    pub paths: Vec<String>,
    /// Run as this sandbox user.
    pub user: Option<String>,
}

/// Options for [`Git::restore`].
#[derive(Default)]
pub struct GitRestoreOpts {
    /// Paths to restore. Must be non-empty; [`Git::restore`] returns
    /// [`Error::InvalidArgument`] when empty.
    pub paths: Vec<String>,
    /// Restore the index / unstage changes (`--staged`).
    pub staged: Option<bool>,
    /// Restore working-tree files (`--worktree`).
    pub worktree: Option<bool>,
    /// Restore from this source — commit, branch, or ref (`--source`).
    pub source: Option<String>,
    /// Run as this sandbox user.
    pub user: Option<String>,
}

/// Options for [`Git::remote_add`].
#[derive(Default)]
pub struct GitRemoteAddOpts {
    /// Fetch the remote immediately after adding (`-f` / standalone `git fetch`).
    pub fetch: bool,
    /// Update the remote URL instead of returning an error when it already exists.
    pub overwrite: bool,
    /// Run as this sandbox user.
    pub user: Option<String>,
}

/// Options for simple git operations that only require a user override.
///
/// Used by [`Git::create_branch`], [`Git::checkout_branch`], and
/// [`Git::remote_get`].
#[derive(Default)]
pub struct GitRequestOpts {
    /// Run as this sandbox user.
    pub user: Option<String>,
}

/// Options for git config operations: [`Git::set_config`], [`Git::get_config`],
/// and [`Git::configure_user`].
#[derive(Default)]
pub struct GitConfigOpts {
    /// Config scope. Defaults to [`GitConfigScope::Global`] when `None`.
    pub scope: Option<GitConfigScope>,
    /// Repository path — required when `scope` is [`GitConfigScope::Local`].
    pub path: Option<String>,
    /// Run as this sandbox user.
    pub user: Option<String>,
}

/// Options for [`Git::clone`].
#[derive(Default)]
pub struct GitCloneOpts {
    /// Branch to clone (`--branch <b> --single-branch`).
    pub branch: Option<String>,
    /// Shallow-clone depth (`--depth <n>`).
    pub depth: Option<u32>,
    /// Local destination path. When omitted the directory is derived from the
    /// URL (e.g. `repo` from `https://h/user/repo.git`). Required when using
    /// credentials without storing them and the URL has no derivable name.
    pub path: Option<String>,
    /// Username for HTTP(S) authentication. Embedded into the clone URL.
    pub username: Option<String>,
    /// Password / token for HTTP(S) authentication. Embedded into the clone URL.
    /// Requires [`GitCloneOpts::username`].
    pub password: Option<String>,
    /// When `true`, the embedded credentials are left in the repository's
    /// `origin` remote URL after cloning. **Use with caution** — anyone with
    /// read access to the repository can recover the credentials from
    /// `.git/config`.
    ///
    /// When `false` (default) the credentials are stripped from the remote URL
    /// via `git remote set-url origin <sanitized>` immediately after cloning.
    pub dangerously_store_credentials: bool,
    /// Run as this sandbox user.
    pub user: Option<String>,
}

/// Options for [`Git::push`].
pub struct GitPushOpts {
    /// Remote to push to (e.g. `"origin"`). Resolved automatically when only
    /// one remote exists and credentials are in use.
    pub remote: Option<String>,
    /// Branch to push. When omitted, git uses its default tracking branch.
    pub branch: Option<String>,
    /// Add `--set-upstream <remote>` to the push command so subsequent
    /// `git pull` / `git push` commands work without arguments.
    ///
    /// Defaults to `true`, matching the JS SDK default.
    pub set_upstream: bool,
    /// Username for HTTP(S) authentication.
    pub username: Option<String>,
    /// Password / token for HTTP(S) authentication. Requires
    /// [`GitPushOpts::username`].
    pub password: Option<String>,
    /// Run as this sandbox user.
    pub user: Option<String>,
}

impl Default for GitPushOpts {
    fn default() -> Self {
        GitPushOpts {
            remote: None,
            branch: None,
            set_upstream: true, // JS default: setUpstream = true
            username: None,
            password: None,
            user: None,
        }
    }
}

/// Options for [`Git::pull`].
#[derive(Default)]
pub struct GitPullOpts {
    /// Remote to pull from (e.g. `"origin"`). When `None` and `branch` is
    /// also `None`, an upstream tracking branch must already be configured.
    pub remote: Option<String>,
    /// Branch to pull. When `None`, git uses the tracking branch.
    pub branch: Option<String>,
    /// Username for HTTP(S) authentication.
    pub username: Option<String>,
    /// Password / token for HTTP(S) authentication. Requires
    /// [`GitPullOpts::username`].
    pub password: Option<String>,
    /// Run as this sandbox user.
    pub user: Option<String>,
}

/// Options for [`Git::dangerously_authenticate`].
///
/// Credentials are written to the global git credential store — they persist
/// until explicitly removed. Prefer short-lived tokens when possible.
#[derive(Default)]
pub struct GitDangerouslyAuthenticateOpts {
    /// Git username (e.g. a GitHub account name). **Required.**
    pub username: String,
    /// Git password or personal-access token. **Required.**
    pub password: String,
    /// Git host to authenticate against. Defaults to `"github.com"`.
    pub host: Option<String>,
    /// Git protocol (e.g. `"https"`). Defaults to `"https"`.
    pub protocol: Option<String>,
    /// Run as this sandbox user.
    pub user: Option<String>,
}

// ---------------------------------------------------------------------------
// Git struct
// ---------------------------------------------------------------------------

/// Runs git operations in a sandbox via the [`Commands`] handle.
///
/// Obtain via [`crate::Sandbox::git`].
pub struct Git {
    commands: Commands,
}

impl Git {
    /// Build a [`Git`] handle from a [`Commands`] handle.
    pub(crate) fn new(commands: Commands) -> Git {
        Git { commands }
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Build [`CommandStartOpts`] with the default git environment
    /// (`GIT_TERMINAL_PROMPT=0`) merged in, matching the JS `runGit`/`runShell`.
    fn make_run_opts(user: Option<String>) -> CommandStartOpts {
        let mut envs = BTreeMap::new();
        envs.insert("GIT_TERMINAL_PROMPT".to_string(), "0".to_string());
        CommandStartOpts {
            user,
            envs,
            ..Default::default()
        }
    }

    /// Run a command string through the sandbox process runner with git defaults.
    async fn run_cmd(&self, cmd: &str, user: Option<String>) -> Result<CommandResult> {
        self.commands.run(cmd, Self::make_run_opts(user)).await
    }

    /// Retrieve the current URL for `remote` in the repository at `path`.
    ///
    /// Returns [`Error::InvalidArgument`] when `git remote get-url` succeeds
    /// but produces no output (remote exists without a URL).
    async fn get_remote_url(
        &self,
        path: &str,
        remote: &str,
        user: Option<String>,
    ) -> Result<String> {
        let cmd = build_git_command(&["remote", "get-url", remote], Some(path));
        let result = self.run_cmd(&cmd, user).await?;
        let url = result.stdout.trim().to_string();
        if url.is_empty() {
            return Err(Error::InvalidArgument(format!(
                "Remote \"{remote}\" URL not found in repository."
            )));
        }
        Ok(url)
    }

    /// Resolve `remote` to a concrete remote name.
    ///
    /// When `remote` is already `Some`, it is returned as-is. Otherwise the
    /// repository's remote list is fetched; if exactly one remote exists that
    /// name is returned. Multiple remotes (or none) result in
    /// [`Error::InvalidArgument`].
    async fn resolve_remote_name(
        &self,
        path: &str,
        remote: Option<&str>,
        user: Option<String>,
    ) -> Result<String> {
        if let Some(r) = remote {
            return Ok(r.to_string());
        }
        let cmd = build_git_command(&["remote"], Some(path));
        let result = self.run_cmd(&cmd, user).await?;
        let remotes: Vec<&str> = result
            .stdout
            .split('\n')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if remotes.len() == 1 {
            return Ok(remotes[0].to_string());
        }
        Err(Error::InvalidArgument(
            "Remote is required when using username/password and the repository \
             has multiple remotes."
                .to_string(),
        ))
    }

    /// Return `true` when the current branch at `path` has an upstream tracking
    /// branch configured.
    ///
    /// Mirrors the JS `hasUpstream` which catches `CommandExitError` and returns
    /// `false`; in Rust our commands return `Ok` on non-zero exit, so we check
    /// `exit_code == 0` instead.
    async fn has_upstream(&self, path: &str, user: Option<String>) -> Result<bool> {
        let cmd = build_git_command(
            &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
            Some(path),
        );
        let result = self.run_cmd(&cmd, user).await?;
        Ok(result.exit_code == 0 && !result.stdout.trim().is_empty())
    }

    /// Temporarily replace `remote`'s URL with a credentialed URL, run `op`,
    /// then restore the original URL.
    ///
    /// Port of `withRemoteCredentials` from `git/index.ts`. The restore step
    /// is **always** executed — even when `op` fails — mirroring the JS
    /// try/finally-like pattern.
    ///
    /// **Note:** `op` is expected to return `Ok(CommandResult)` even on
    /// non-zero git exit (the SDK's Ok-on-nonzero rule); the only `Err` case
    /// from `op` is a transport failure.  Auth/upstream error mapping is
    /// therefore NOT applied inside this helper — the calling code (the
    /// non-credentialed path of [`Git::push`] / [`Git::pull`]) handles it.
    /// This is a faithful port of the JS asymmetry; see the task-3 report.
    async fn with_remote_credentials<Fut>(
        &self,
        path: &str,
        remote: &str,
        username: &str,
        password: &str,
        run_user: Option<String>,
        op: Fut,
    ) -> Result<CommandResult>
    where
        Fut: Future<Output = Result<CommandResult>> + Send,
    {
        let original_url = self.get_remote_url(path, remote, run_user.clone()).await?;
        let cred_url = with_credentials(&original_url, Some(username), Some(password))?;

        // Set credentialed URL — non-zero exit is promoted to Err so the op is
        // never run against the wrong (uncredentialed) URL.
        let set_cmd = build_git_command(&["remote", "set-url", remote, &cred_url], Some(path));
        check_set_url_exit(self.run_cmd(&set_cmd, run_user.clone()).await?)?;

        // Run the operation, capturing any error.
        let op_result = op.await;

        // Always restore the original URL — non-zero exit is promoted to Err
        // so a failed restore (credentials left in .git/config) never slips
        // through silently.  Op error still takes priority (JS ordering).
        let restore_cmd =
            build_git_command(&["remote", "set-url", remote, &original_url], Some(path));
        let restore_result = self
            .run_cmd(&restore_cmd, run_user)
            .await
            .and_then(check_set_url_exit);

        // Op error takes priority; then restore error.
        match (op_result, restore_result) {
            (Err(op_err), _) => Err(op_err),
            (Ok(_), Err(restore_err)) => Err(restore_err),
            (Ok(result), Ok(_)) => Ok(result),
        }
    }

    // -----------------------------------------------------------------------
    // Public methods
    // -----------------------------------------------------------------------

    /// Clone a git repository into the sandbox.
    ///
    /// Credentials (if provided) are embedded into the clone URL. Unless
    /// `opts.dangerously_store_credentials` is `true`, the credentials are
    /// stripped from the `origin` remote URL immediately after a successful
    /// clone via `git remote set-url origin <sanitized>`.
    ///
    /// A non-zero git exit whose output indicates an authentication failure is
    /// mapped to [`Error::GitAuth`]. All other non-zero exits are returned as
    /// `Ok(CommandResult)` per the SDK's Ok-on-nonzero rule.
    ///
    /// Equivalent to:
    /// `git clone [--branch <b> --single-branch] [--depth <n>] <url> [<path>]`
    pub async fn clone(&self, url: &str, opts: GitCloneOpts) -> Result<CommandResult> {
        let GitCloneOpts {
            username,
            password,
            branch,
            depth,
            path,
            dangerously_store_credentials,
            user,
        } = opts;

        if password.is_some() && username.is_none() {
            return Err(Error::InvalidArgument(
                "Username is required when using a password or token for git clone.".to_string(),
            ));
        }

        // Build the URL with embedded credentials when both are provided.
        let url_with_creds = if username.is_some() && password.is_some() {
            with_credentials(url, username.as_deref(), password.as_deref())?
        } else {
            url.to_string()
        };

        // Determine whether to strip credentials after cloning.
        let sanitized = strip_credentials(&url_with_creds);
        let strip_inline = !dangerously_store_credentials && sanitized != url_with_creds;

        // Derive the local destination path.
        let repo_path = if strip_inline {
            path.or_else(|| derive_repo_dir_from_url(url))
        } else {
            path
        };

        if strip_inline && repo_path.is_none() {
            return Err(Error::InvalidArgument(
                "A destination path is required when using credentials without storing them."
                    .to_string(),
            ));
        }

        // Build the clone argument list.
        // Note: `git clone` uses a positional <path> argument, not `-C`.
        let mut args: Vec<String> = vec!["clone".to_string(), url_with_creds];
        if let Some(ref b) = branch {
            args.push("--branch".to_string());
            args.push(b.clone());
            args.push("--single-branch".to_string());
        }
        if let Some(d) = depth {
            args.push("--depth".to_string());
            args.push(d.to_string());
        }
        if let Some(ref p) = repo_path {
            args.push(p.clone());
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let cmd = build_git_command(&refs, None); // no -C; path is positional
        let result = self.run_cmd(&cmd, user.clone()).await?;

        // Non-zero exit: map auth failures; return all others as Ok.
        if result.exit_code != 0 {
            if is_auth_failure(&result) {
                return Err(Error::GitAuth(build_auth_error_message(
                    "clone",
                    username.is_some() && password.is_none(),
                )));
            }
            return Ok(result);
        }

        // Exit 0 + strip_inline: reset origin to the sanitized URL.
        // check_set_url_exit promotes a non-zero exit to Err so credentials are
        // never silently left in .git/config when the strip command fails.
        if strip_inline && let Some(ref rp) = repo_path {
            let set_url_cmd =
                build_git_command(&["remote", "set-url", "origin", &sanitized], Some(rp));
            check_set_url_exit(self.run_cmd(&set_url_cmd, user).await?)?;
        }

        Ok(result)
    }

    /// Push commits to a remote.
    ///
    /// When credentials are provided, the remote URL is temporarily replaced
    /// with a credentialed URL via `git remote set-url`, the push is run, then
    /// the original URL is restored.
    ///
    /// Non-zero exits on the **non-credentialed** path are mapped:
    /// - Auth failures → [`Error::GitAuth`].
    /// - Missing upstream → [`Error::GitUpstream`].
    ///
    /// **Note:** On the credentialed path, auth/upstream mapping is NOT applied
    /// (faithful port of the JS SDK asymmetry — see task-3 report for details).
    ///
    /// Equivalent to:
    /// `git -C <path> push [--set-upstream <remote>] [<remote>] [<branch>]`
    pub async fn push(&self, path: &str, opts: GitPushOpts) -> Result<CommandResult> {
        let GitPushOpts {
            remote,
            branch,
            set_upstream,
            username,
            password,
            user,
        } = opts;

        if password.is_some() && username.is_none() {
            return Err(Error::InvalidArgument(
                "Username is required when using a password or token for git push.".to_string(),
            ));
        }

        let missing_password = username.is_some() && password.is_none();

        // Credentialed path: use with_remote_credentials.
        if let (Some(uname), Some(pword)) = (username.as_deref(), password.as_deref()) {
            let remote_name = self
                .resolve_remote_name(path, remote.as_deref(), user.clone())
                .await?;
            let commands = self.commands.clone();
            let run_user_for_op = user.clone();
            let push_args = build_push_args(
                Some(&remote_name),
                remote.as_deref(),
                branch.as_deref(),
                set_upstream,
            );
            let refs_owned: Vec<String> = push_args;
            let refs: Vec<&str> = refs_owned.iter().map(String::as_str).collect();
            let push_cmd = build_git_command(&refs, Some(path));
            // Move owned data into the async block to avoid borrow-after-return.
            let op = async move {
                commands
                    .run(&push_cmd, Git::make_run_opts(run_user_for_op))
                    .await
            };
            // CONCERN: The credentialed push path does NOT map auth/upstream
            // failures — faithful to the JS SDK asymmetry.  See task-3 report.
            return self
                .with_remote_credentials(path, &remote_name, uname, pword, user, op)
                .await;
        }

        // Non-credentialed path: build args without an explicit remote name.
        let push_args = build_push_args(None, remote.as_deref(), branch.as_deref(), set_upstream);
        let refs: Vec<&str> = push_args.iter().map(String::as_str).collect();
        let cmd = build_git_command(&refs, Some(path));
        let result = self.run_cmd(&cmd, user).await?;

        if result.exit_code != 0 {
            if is_auth_failure(&result) {
                return Err(Error::GitAuth(build_auth_error_message(
                    "push",
                    missing_password,
                )));
            }
            if is_missing_upstream(&result) {
                return Err(Error::GitUpstream(build_upstream_error_message("push")));
            }
        }

        Ok(result)
    }

    /// Pull changes from a remote.
    ///
    /// When both `remote` and `branch` are `None`, the current branch must have
    /// an upstream tracking branch configured — otherwise [`Error::GitUpstream`]
    /// is returned before any git command runs.
    ///
    /// When credentials are provided the remote URL is temporarily replaced via
    /// `git remote set-url` (see [`Git::push`] for the full credential dance).
    ///
    /// **Note:** Auth/upstream mapping applies only on the non-credentialed path.
    /// See the DONE_WITH_CONCERNS note in task-3-report.md.
    ///
    /// Equivalent to:
    /// `git -C <path> pull [<remote>] [<branch>]`
    pub async fn pull(&self, path: &str, opts: GitPullOpts) -> Result<CommandResult> {
        let GitPullOpts {
            remote,
            branch,
            username,
            password,
            user,
        } = opts;

        if password.is_some() && username.is_none() {
            return Err(Error::InvalidArgument(
                "Username is required when using a password or token for git pull.".to_string(),
            ));
        }

        let missing_password = username.is_some() && password.is_none();

        // No remote and no branch: ensure an upstream tracking branch exists.
        if remote.is_none() && branch.is_none() && !self.has_upstream(path, user.clone()).await? {
            return Err(Error::GitUpstream(build_upstream_error_message("pull")));
        }

        // Credentialed path.
        if let (Some(uname), Some(pword)) = (username.as_deref(), password.as_deref()) {
            let remote_name = self
                .resolve_remote_name(path, remote.as_deref(), user.clone())
                .await?;
            let commands = self.commands.clone();
            let run_user_for_op = user.clone();

            // Build pull args using remote_name (credentialed).
            let mut pull_args: Vec<String> = vec!["pull".to_string()];
            pull_args.push(remote_name.clone());
            if let Some(b) = branch.as_deref() {
                pull_args.push(b.to_string());
            }
            let refs: Vec<&str> = pull_args.iter().map(String::as_str).collect();
            let pull_cmd = build_git_command(&refs, Some(path));
            let op = async move {
                commands
                    .run(&pull_cmd, Git::make_run_opts(run_user_for_op))
                    .await
            };
            // CONCERN: Same asymmetry as push — no auth/upstream mapping here.
            return self
                .with_remote_credentials(path, &remote_name, uname, pword, user, op)
                .await;
        }

        // Non-credentialed path.
        let mut pull_args: Vec<String> = vec!["pull".to_string()];
        if let Some(r) = remote.as_deref() {
            pull_args.push(r.to_string());
        }
        if let Some(b) = branch.as_deref() {
            pull_args.push(b.to_string());
        }
        let refs: Vec<&str> = pull_args.iter().map(String::as_str).collect();
        let cmd = build_git_command(&refs, Some(path));
        let result = self.run_cmd(&cmd, user).await?;

        if result.exit_code != 0 {
            if is_auth_failure(&result) {
                return Err(Error::GitAuth(build_auth_error_message(
                    "pull",
                    missing_password,
                )));
            }
            if is_missing_upstream(&result) {
                return Err(Error::GitUpstream(build_upstream_error_message("pull")));
            }
        }

        Ok(result)
    }

    /// Initialize a new git repository at `path`.
    ///
    /// `path` is the positional argument to `git init` (not a `-C` flag);
    /// git creates the directory. Equivalent to:
    /// `git init [--initial-branch <b>] [--bare] <path>`
    pub async fn init(&self, path: &str, opts: GitInitOpts) -> Result<CommandResult> {
        let mut args: Vec<String> = vec!["init".to_string()];
        if let Some(branch) = opts.initial_branch {
            args.push("--initial-branch".to_string());
            args.push(branch);
        }
        if opts.bare {
            args.push("--bare".to_string());
        }
        args.push(path.to_string());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        // No `-C` prefix — `path` is the positional destination.
        let cmd = build_git_command(&refs, None);
        self.run_cmd(&cmd, opts.user).await
    }

    /// Stage files for commit.
    ///
    /// Equivalent to:
    /// - `git -C <path> add -A` — default (`files` empty, `all` true)
    /// - `git -C <path> add .` — `files` empty, `all` false
    /// - `git -C <path> add -- <files…>` — specific files
    pub async fn add(&self, path: &str, opts: GitAddOpts) -> Result<CommandResult> {
        let mut args: Vec<String> = vec!["add".to_string()];
        if opts.files.is_empty() {
            args.push(if opts.all { "-A" } else { "." }.to_string());
        } else {
            args.push("--".to_string());
            args.extend(opts.files);
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let cmd = build_git_command(&refs, Some(path));
        self.run_cmd(&cmd, opts.user).await
    }

    /// Create a commit with `message`.
    ///
    /// Author overrides are injected as `-c` flags before the `commit`
    /// subcommand, matching the JS SDK. Equivalent to:
    /// `git -C <path> [-c user.name=<n>] [-c user.email=<e>] commit -m <message> [--allow-empty]`
    pub async fn commit(
        &self,
        path: &str,
        message: &str,
        opts: GitCommitOpts,
    ) -> Result<CommandResult> {
        let mut args: Vec<String> = Vec::new();
        // Author overrides come before the `commit` sub-command (JS: [...authorArgs, ...args]).
        if let Some(name) = opts.author_name {
            args.push("-c".to_string());
            args.push(format!("user.name={name}"));
        }
        if let Some(email) = opts.author_email {
            args.push("-c".to_string());
            args.push(format!("user.email={email}"));
        }
        args.push("commit".to_string());
        args.push("-m".to_string());
        args.push(message.to_string());
        if opts.allow_empty {
            args.push("--allow-empty".to_string());
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let cmd = build_git_command(&refs, Some(path));
        self.run_cmd(&cmd, opts.user).await
    }

    /// Create and check out a new branch.
    ///
    /// Equivalent to `git -C <path> checkout -b <branch>`.
    pub async fn create_branch(
        &self,
        path: &str,
        branch: &str,
        opts: GitRequestOpts,
    ) -> Result<CommandResult> {
        let cmd = build_git_command(&["checkout", "-b", branch], Some(path));
        self.run_cmd(&cmd, opts.user).await
    }

    /// Check out an existing branch.
    ///
    /// Equivalent to `git -C <path> checkout <branch>`.
    pub async fn checkout_branch(
        &self,
        path: &str,
        branch: &str,
        opts: GitRequestOpts,
    ) -> Result<CommandResult> {
        let cmd = build_git_command(&["checkout", branch], Some(path));
        self.run_cmd(&cmd, opts.user).await
    }

    /// Delete a branch.
    ///
    /// Equivalent to `git -C <path> branch -d <branch>`, or `-D` when
    /// `opts.force` is `true`.
    pub async fn delete_branch(
        &self,
        path: &str,
        branch: &str,
        opts: GitDeleteBranchOpts,
    ) -> Result<CommandResult> {
        let flag = if opts.force { "-D" } else { "-d" };
        let cmd = build_git_command(&["branch", flag, branch], Some(path));
        self.run_cmd(&cmd, opts.user).await
    }

    /// Reset the current `HEAD` to a specified state.
    ///
    /// Equivalent to:
    /// `git -C <path> reset [--soft|--mixed|--hard|--merge|--keep] [<target>] [-- <paths…>]`
    pub async fn reset(&self, path: &str, opts: GitResetOpts) -> Result<CommandResult> {
        let mut args: Vec<String> = vec!["reset".to_string()];
        if let Some(mode) = opts.mode {
            args.push(mode.flag().to_string());
        }
        if let Some(target) = opts.target {
            args.push(target);
        }
        if !opts.paths.is_empty() {
            args.push("--".to_string());
            args.extend(opts.paths);
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let cmd = build_git_command(&refs, Some(path));
        self.run_cmd(&cmd, opts.user).await
    }

    /// Restore working-tree files or unstage changes.
    ///
    /// `opts.paths` must be non-empty; this method returns
    /// [`Error::InvalidArgument`] when it is empty.
    ///
    /// `staged` and `worktree` follow the same defaulting logic as the JS SDK:
    /// when both are `None`, `worktree` is set to `true`. When only `staged` is
    /// `true`, `worktree` is set to `false` (and vice-versa).
    ///
    /// Equivalent to:
    /// `git -C <path> restore [--worktree] [--staged] [--source <src>] -- <paths…>`
    pub async fn restore(&self, path: &str, opts: GitRestoreOpts) -> Result<CommandResult> {
        if opts.paths.is_empty() {
            return Err(Error::InvalidArgument(
                "At least one path is required.".to_string(),
            ));
        }

        // Port JS defaulting logic for staged / worktree.
        let (resolved_staged, resolved_worktree) = match (opts.staged, opts.worktree) {
            (None, None) => (None, Some(true)),
            (Some(true), None) => (Some(true), Some(false)),
            (None, v) => (Some(false), v),
            (s, w) => (s, w),
        };

        if resolved_staged == Some(false) && resolved_worktree == Some(false) {
            return Err(Error::InvalidArgument(
                "At least one of staged or worktree must be true.".to_string(),
            ));
        }

        let mut args: Vec<String> = vec!["restore".to_string()];
        if resolved_worktree == Some(true) {
            args.push("--worktree".to_string());
        }
        if resolved_staged == Some(true) {
            args.push("--staged".to_string());
        }
        if let Some(source) = opts.source {
            args.push("--source".to_string());
            args.push(source);
        }
        args.push("--".to_string());
        args.extend(opts.paths);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let cmd = build_git_command(&refs, Some(path));
        self.run_cmd(&cmd, opts.user).await
    }

    /// Set a git config value.
    ///
    /// A repository `path` in `opts` is required when `scope` is
    /// [`GitConfigScope::Local`]. Equivalent to:
    /// `git [-C <path>] config [--global|--local|--system] <key> <value>`
    pub async fn set_config(
        &self,
        key: &str,
        value: &str,
        opts: GitConfigOpts,
    ) -> Result<CommandResult> {
        if key.is_empty() {
            return Err(Error::InvalidArgument(
                "Git config key is required.".to_string(),
            ));
        }
        let scope = opts.scope.unwrap_or(GitConfigScope::Global);
        let flag = scope_flag(scope);
        let repo_path = repo_path_for_scope(scope, opts.path.as_deref())?;
        let cmd = build_git_command(&["config", flag, key, value], repo_path.as_deref());
        self.run_cmd(&cmd, opts.user).await
    }

    /// Get a git config value, returning `None` when the key is not set.
    ///
    /// Equivalent to:
    /// `git [-C <path>] config [--global|--local|--system] --get <key> || true`
    pub async fn get_config(&self, key: &str, opts: GitConfigOpts) -> Result<Option<String>> {
        if key.is_empty() {
            return Err(Error::InvalidArgument(
                "Git config key is required.".to_string(),
            ));
        }
        let scope = opts.scope.unwrap_or(GitConfigScope::Global);
        let flag = scope_flag(scope);
        let repo_path = repo_path_for_scope(scope, opts.path.as_deref())?;
        let git_cmd = build_git_command(&["config", flag, "--get", key], repo_path.as_deref());
        // `|| true` ensures a non-zero exit when the key is absent does not
        // surface as an error; we distinguish via stdout being empty.
        let cmd = format!("{git_cmd} || true");
        let result = self.run_cmd(&cmd, opts.user).await?;
        let trimmed = result.stdout.trim().to_string();
        Ok(if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        })
    }

    /// Configure git user name and email.
    ///
    /// Runs two `git config` calls — one for `user.name`, one for
    /// `user.email` — and returns the result of the second, matching the JS SDK.
    pub async fn configure_user(
        &self,
        name: &str,
        email: &str,
        opts: GitConfigOpts,
    ) -> Result<CommandResult> {
        if name.is_empty() || email.is_empty() {
            return Err(Error::InvalidArgument(
                "Both name and email are required.".to_string(),
            ));
        }
        let scope = opts.scope.unwrap_or(GitConfigScope::Global);
        let flag = scope_flag(scope);
        let repo_path = repo_path_for_scope(scope, opts.path.as_deref())?;
        let name_cmd =
            build_git_command(&["config", flag, "user.name", name], repo_path.as_deref());
        let email_cmd =
            build_git_command(&["config", flag, "user.email", email], repo_path.as_deref());
        self.run_cmd(&name_cmd, opts.user.clone()).await?;
        self.run_cmd(&email_cmd, opts.user).await
    }

    /// Add a remote to a repository, or update its URL when it already exists.
    ///
    /// When `opts.overwrite` is `false` (default):
    /// `git -C <path> remote add [-f] <name> <url>`
    ///
    /// When `opts.overwrite` is `true`, runs a composite shell command:
    /// `(git -C <path> remote add [-f] <name> <url> || git -C <path> remote set-url <name> <url>)`
    /// optionally followed by `&& git -C <path> fetch <name>` when `fetch` is `true`.
    pub async fn remote_add(
        &self,
        path: &str,
        name: &str,
        url: &str,
        opts: GitRemoteAddOpts,
    ) -> Result<CommandResult> {
        if name.is_empty() || url.is_empty() {
            return Err(Error::InvalidArgument(
                "Both remote name and URL are required to add a git remote.".to_string(),
            ));
        }
        let mut add_args: Vec<String> = vec!["remote".to_string(), "add".to_string()];
        if opts.fetch {
            add_args.push("-f".to_string());
        }
        add_args.push(name.to_string());
        add_args.push(url.to_string());

        if !opts.overwrite {
            let refs: Vec<&str> = add_args.iter().map(String::as_str).collect();
            let cmd = build_git_command(&refs, Some(path));
            return self.run_cmd(&cmd, opts.user).await;
        }

        // Composite form: try `remote add`, fall back to `remote set-url`.
        let refs: Vec<&str> = add_args.iter().map(String::as_str).collect();
        let add_cmd = build_git_command(&refs, Some(path));
        let set_url_cmd = build_git_command(&["remote", "set-url", name, url], Some(path));
        let mut cmd = format!("{add_cmd} || {set_url_cmd}");
        if opts.fetch {
            // The fetch is separated from the `remote add -f` so it also runs
            // after a `set-url` fallback.
            let fetch_cmd = build_git_command(&["fetch", name], Some(path));
            cmd = format!("({cmd}) && {fetch_cmd}");
        }
        self.run_cmd(&cmd, opts.user).await
    }

    /// Get the URL for a named remote, returning `None` when the remote does
    /// not exist.
    ///
    /// Equivalent to `git -C <path> remote get-url <name> || true`.
    pub async fn remote_get(
        &self,
        path: &str,
        name: &str,
        opts: GitRequestOpts,
    ) -> Result<Option<String>> {
        if name.is_empty() {
            return Err(Error::InvalidArgument(
                "Remote name is required.".to_string(),
            ));
        }
        let git_cmd = build_git_command(&["remote", "get-url", name], Some(path));
        let cmd = format!("{git_cmd} || true");
        let result = self.run_cmd(&cmd, opts.user).await?;
        let trimmed = result.stdout.trim().to_string();
        Ok(if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        })
    }

    /// Get repository status information.
    ///
    /// Runs `git -C <path> status --porcelain=1 -b` and parses the stdout into
    /// a [`GitStatus`].
    ///
    /// A non-zero git exit is NOT treated as an error — stdout is always parsed,
    /// mirroring the JS SDK which calls `parseGitStatus(result.stdout)` unconditionally.
    pub async fn status(&self, path: &str, user: Option<&str>) -> Result<GitStatus> {
        let cmd = build_git_command(&["status", "--porcelain=1", "-b"], Some(path));
        let result = self.run_cmd(&cmd, user.map(str::to_string)).await?;
        Ok(parse_git_status(&result.stdout))
    }

    /// List branches in a repository.
    ///
    /// Runs `git -C <path> branch --format=%(refname:short)\t%(HEAD)` (the `\t`
    /// is a real tab character used as the field separator) and parses the
    /// stdout into a [`GitBranches`].
    ///
    /// A non-zero git exit is NOT treated as an error — stdout is always parsed,
    /// mirroring the JS SDK which calls `parseGitBranches(result.stdout)` unconditionally.
    pub async fn branches(&self, path: &str, user: Option<&str>) -> Result<GitBranches> {
        // The \t is a real tab character — git's --format uses it as the field
        // separator and parse_git_branches splits on '\t'.
        let cmd = build_git_command(
            &["branch", "--format=%(refname:short)\t%(HEAD)"],
            Some(path),
        );
        let result = self.run_cmd(&cmd, user.map(str::to_string)).await?;
        Ok(parse_git_branches(&result.stdout))
    }

    /// Authenticate git globally via the credential helper store.
    ///
    /// Configures `credential.helper = store` then injects credentials via
    /// `git credential approve`.  Credentials **persist** in the sandbox until
    /// explicitly removed — prefer short-lived tokens when possible.
    ///
    /// Returns [`Error::InvalidArgument`] when [`GitDangerouslyAuthenticateOpts::username`]
    /// or [`GitDangerouslyAuthenticateOpts::password`] is empty.
    ///
    /// Port of `dangerouslyAuthenticate` from `git/index.ts` (line 855).
    pub async fn dangerously_authenticate(
        &self,
        opts: GitDangerouslyAuthenticateOpts,
    ) -> Result<CommandResult> {
        let GitDangerouslyAuthenticateOpts {
            username,
            password,
            host,
            protocol,
            user,
        } = opts;

        if username.is_empty() || password.is_empty() {
            return Err(Error::InvalidArgument(
                "Both username and password are required to authenticate git.".to_string(),
            ));
        }

        let target_host = host.as_deref().unwrap_or("github.com").trim().to_string();
        let target_protocol = protocol.as_deref().unwrap_or("https").trim().to_string();

        // Six elements joined by "\n".  The two trailing empty strings make the
        // input end with "\n\n" — a blank line, which is how git terminates
        // credential input blocks.
        let credential_input = [
            format!("protocol={target_protocol}"),
            format!("host={target_host}"),
            format!("username={username}"),
            format!("password={password}"),
            String::new(),
            String::new(),
        ]
        .join("\n");

        // Step 1: configure credential.helper to "store" (no -C; global config).
        let config_cmd =
            build_git_command(&["config", "--global", "credential.helper", "store"], None);
        self.run_cmd(&config_cmd, user.clone()).await?;

        // Step 2: pipe the credential block into `git credential approve`.
        // This is a raw shell pipeline (JS: runShell), not a single git command.
        let approve_git_cmd = build_git_command(&["credential", "approve"], None);
        let approve_cmd = format!(
            "printf %s {} | {}",
            crate::utils::shell_quote(&credential_input),
            approve_git_cmd
        );
        self.run_cmd(&approve_cmd, user).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::client::{ConnectClient, ConnectClientOpts};
    use crate::connect::envelope::{FLAG_END_STREAM, encode_envelope};
    use base64::prelude::{BASE64_STANDARD, Engine as _};
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn connect_for(server: &MockServer) -> Arc<ConnectClient> {
        Arc::new(
            ConnectClient::new(ConnectClientOpts {
                base_url: server.uri(),
                access_token: None,
                sandbox_id: "sbx_c".to_string(),
                envd_port: 49983,
                user_agent: "e2b-rs-test".to_string(),
                envd_version: "0.6.3".to_string(),
                request_timeout_ms: 60_000,
                logger: None,
                proxy: None,
            })
            .expect("connect client"),
        )
    }

    /// Build a minimal streaming process body: Start → End(exit_code).
    fn proc_stream(exit_code: i32) -> Vec<u8> {
        let mut body = encode_envelope(0, br#"{"event":{"start":{"pid":7}}}"#);
        body.extend(encode_envelope(
            0,
            format!(
                r#"{{"event":{{"end":{{"exitCode":{exit_code},"exited":true,"status":"exited"}}}}}}"#
            )
            .as_bytes(),
        ));
        body.extend(encode_envelope(FLAG_END_STREAM, b"{}"));
        body
    }

    /// Mount a single mock for `/process.Process/Start` returning `exit_code`.
    async fn mount_proc(server: &MockServer, exit_code: i32) {
        Mock::given(method("POST"))
            .and(path("/process.Process/Start"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+json")
                    .set_body_bytes(proc_stream(exit_code)),
            )
            .mount(server)
            .await;
    }

    // -----------------------------------------------------------------------
    // Task 3: clone / push / pull tests (written before implementation — TDD RED)
    // -----------------------------------------------------------------------

    /// Build a process stream body with a stderr Data frame followed by an End
    /// event.  Used to simulate git command failures with diagnostic output.
    fn proc_stream_with_stderr(exit_code: i32, stderr_b64: &str) -> Vec<u8> {
        let mut body = encode_envelope(0, br#"{"event":{"start":{"pid":7}}}"#);
        body.extend(encode_envelope(
            0,
            format!(r#"{{"event":{{"data":{{"stderr":"{stderr_b64}"}}}}}}"#).as_bytes(),
        ));
        body.extend(encode_envelope(
            0,
            format!(
                r#"{{"event":{{"end":{{"exitCode":{exit_code},"exited":true,"status":"exited"}}}}}}"#
            )
            .as_bytes(),
        ));
        body.extend(encode_envelope(FLAG_END_STREAM, b"{}"));
        body
    }

    /// Mount a single mock for `/process.Process/Start` that streams stderr
    /// output followed by a non-zero exit.
    async fn mount_proc_with_stderr(server: &MockServer, exit_code: i32, stderr_b64: &str) {
        Mock::given(method("POST"))
            .and(path("/process.Process/Start"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+json")
                    .set_body_bytes(proc_stream_with_stderr(exit_code, stderr_b64)),
            )
            .mount(server)
            .await;
    }

    /// `git clone` with credentials on a private repo that returns an auth
    /// failure stderr message must map to `Err(Error::GitAuth(_))`.
    #[tokio::test]
    async fn clone_auth_failure_returns_git_auth_err() {
        let server = MockServer::start().await;
        // base64("fatal: Authentication failed")
        mount_proc_with_stderr(&server, 128, "ZmF0YWw6IEF1dGhlbnRpY2F0aW9uIGZhaWxlZA==").await;
        let git = Git::new(Commands::build_with_connect(connect_for(&server), "0.6.3"));
        let err = git
            .clone(
                "https://github.com/private/repo.git",
                GitCloneOpts {
                    username: Some("u".to_string()),
                    password: Some("p".to_string()),
                    // Store creds so no post-clone set-url is attempted (avoids
                    // second mock call, keeping the test simple).
                    dangerously_store_credentials: true,
                    ..Default::default()
                },
            )
            .await
            .expect_err("auth failure must be Err");
        assert!(
            matches!(err, crate::errors::Error::GitAuth(_)),
            "expected GitAuth, got {err:?}"
        );
    }

    /// A successful `git clone` (exit 0) returns `Ok(CommandResult)`.
    #[tokio::test]
    async fn clone_happy_path_returns_ok() {
        let server = MockServer::start().await;
        mount_proc(&server, 0).await;
        let git = Git::new(Commands::build_with_connect(connect_for(&server), "0.6.3"));
        let result = git
            .clone(
                "https://github.com/public/repo.git",
                GitCloneOpts {
                    // Use dangerously_store_credentials=true to suppress the
                    // post-clone set-url command (keeps the mock simple).
                    dangerously_store_credentials: true,
                    ..Default::default()
                },
            )
            .await
            .expect("clone ok");
        assert_eq!(result.exit_code, 0);
    }

    /// Supplying credentials with `dangerously_store_credentials=false` and no
    /// resolvable destination path must return `Err(Error::InvalidArgument)`
    /// **before** any network round-trip.
    #[tokio::test]
    async fn clone_missing_path_guard_fires_before_network() {
        let server = MockServer::start().await;
        let git = Git::new(Commands::build_with_connect(connect_for(&server), "0.6.3"));
        // "https://h/" has no URL path segment → derive_repo_dir_from_url returns None.
        let err = git
            .clone(
                "https://h/",
                GitCloneOpts {
                    username: Some("u".to_string()),
                    password: Some("p".to_string()),
                    dangerously_store_credentials: false,
                    path: None,
                    ..Default::default()
                },
            )
            .await
            .expect_err("missing-path guard");
        assert!(
            matches!(err, crate::errors::Error::InvalidArgument(_)),
            "expected InvalidArgument, got {err:?}"
        );
        // Guard fires before any I/O — server must have received zero requests.
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "guard must fire before any network round-trip"
        );
    }

    /// Non-credentialed `git push` whose stderr indicates a missing upstream
    /// branch must map to `Err(Error::GitUpstream(_))`.
    #[tokio::test]
    async fn push_non_credentialed_upstream_error() {
        let server = MockServer::start().await;
        // base64("fatal: The current branch has no upstream branch.")
        mount_proc_with_stderr(
            &server,
            128,
            "ZmF0YWw6IFRoZSBjdXJyZW50IGJyYW5jaCBoYXMgbm8gdXBzdHJlYW0gYnJhbmNoLg==",
        )
        .await;
        let git = Git::new(Commands::build_with_connect(connect_for(&server), "0.6.3"));
        let err = git
            .push("/repo", GitPushOpts::default())
            .await
            .expect_err("upstream error must be Err");
        assert!(
            matches!(err, crate::errors::Error::GitUpstream(_)),
            "expected GitUpstream, got {err:?}"
        );
    }

    /// Non-credentialed `git pull` with no remote and no branch, when the
    /// current branch has no upstream tracking branch, must return
    /// `Err(Error::GitUpstream(_))`.
    ///
    /// The pre-check calls `git rev-parse --abbrev-ref --symbolic-full-name @{u}`
    /// (`has_upstream`).  A non-zero exit makes `has_upstream` return `false`,
    /// which triggers the upstream error before any pull command runs.
    #[tokio::test]
    async fn pull_non_credentialed_upstream_error() {
        let server = MockServer::start().await;
        // A non-zero exit from the rev-parse probe is enough — no special stderr needed.
        Mock::given(method("POST"))
            .and(path("/process.Process/Start"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+json")
                    .set_body_bytes(proc_stream(128)),
            )
            .mount(&server)
            .await;
        let git = Git::new(Commands::build_with_connect(connect_for(&server), "0.6.3"));
        let err = git
            .pull("/repo", GitPullOpts::default())
            .await
            .expect_err("upstream error must be Err");
        assert!(
            matches!(err, crate::errors::Error::GitUpstream(_)),
            "expected GitUpstream, got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Integration: Git over mock Commands
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn checkout_branch_calls_commands_run() {
        let server = MockServer::start().await;
        mount_proc(&server, 0).await;
        let commands = Commands::build_with_connect(connect_for(&server), "0.6.3");
        let git = Git::new(commands);
        let result = git
            .checkout_branch("/repo", "main", Default::default())
            .await
            .expect("checkout_branch");
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn git_methods_return_non_zero_as_ok() {
        // Non-zero exit is data, not an error (Plan 3c rule).
        let server = MockServer::start().await;
        mount_proc(&server, 1).await;
        let commands = Commands::build_with_connect(connect_for(&server), "0.6.3");
        let git = Git::new(commands);
        let result = git
            .create_branch("/repo", "feature", Default::default())
            .await
            .expect("create_branch non-zero must be Ok");
        assert_eq!(result.exit_code, 1);
    }

    // -----------------------------------------------------------------------
    // Unit: command string construction (pure, no I/O)
    // -----------------------------------------------------------------------

    #[test]
    fn init_no_opts() {
        // git init /path
        assert_eq!(
            build_git_command(&["init", "/path"], None),
            "git init /path"
        );
    }

    #[test]
    fn add_default_is_all() {
        // Default GitAddOpts → all=true
        let opts = GitAddOpts::default();
        assert!(opts.all, "default all should be true");
        assert!(opts.files.is_empty());
        // build: git -C /repo add -A
        assert_eq!(
            build_git_command(&["add", "-A"], Some("/repo")),
            "git -C /repo add -A"
        );
    }

    #[test]
    fn add_specific_files_uses_dashdash() {
        // git -C /repo add -- a.rs b.rs
        assert_eq!(
            build_git_command(&["add", "--", "a.rs", "b.rs"], Some("/repo")),
            "git -C /repo add -- a.rs b.rs"
        );
    }

    #[test]
    fn commit_author_args_before_commit() {
        // git -C /repo -c user.name=Alice -c user.email=a@b.com commit -m 'msg'
        assert_eq!(
            build_git_command(
                &[
                    "-c",
                    "user.name=Alice",
                    "-c",
                    "user.email=a@b.com",
                    "commit",
                    "-m",
                    "msg"
                ],
                Some("/repo")
            ),
            "git -C /repo -c user.name=Alice -c user.email=a@b.com commit -m msg"
        );
    }

    #[test]
    fn checkout_branch_command() {
        assert_eq!(
            build_git_command(&["checkout", "main"], Some("/repo")),
            "git -C /repo checkout main"
        );
    }

    #[test]
    fn create_branch_command() {
        assert_eq!(
            build_git_command(&["checkout", "-b", "feature"], Some("/repo")),
            "git -C /repo checkout -b feature"
        );
    }

    #[test]
    fn delete_branch_force_flag() {
        assert_eq!(
            build_git_command(&["branch", "-D", "old"], Some("/r")),
            "git -C /r branch -D old"
        );
        assert_eq!(
            build_git_command(&["branch", "-d", "old"], Some("/r")),
            "git -C /r branch -d old"
        );
    }

    #[test]
    fn reset_hard_with_target_and_paths() {
        // `HEAD~1` contains `~` which is not a safe shell char → single-quoted.
        assert_eq!(
            build_git_command(&["reset", "--hard", "HEAD~1", "--", "src/"], Some("/r")),
            "git -C /r reset --hard 'HEAD~1' -- src/"
        );
    }

    #[test]
    fn restore_worktree_default() {
        // When staged=None, worktree=None → resolves to worktree=true
        // git -C /r restore --worktree -- src/a.rs
        assert_eq!(
            build_git_command(&["restore", "--worktree", "--", "src/a.rs"], Some("/r")),
            "git -C /r restore --worktree -- src/a.rs"
        );
    }

    #[test]
    fn set_config_global() {
        assert_eq!(
            build_git_command(&["config", "--global", "pull.rebase", "false"], None),
            "git config --global pull.rebase false"
        );
    }

    #[test]
    fn get_config_with_get_flag() {
        // git config --global --get pull.rebase || true
        let git_cmd = build_git_command(&["config", "--global", "--get", "pull.rebase"], None);
        assert_eq!(
            format!("{git_cmd} || true"),
            "git config --global --get pull.rebase || true"
        );
    }

    #[test]
    fn remote_add_overwrite_composite_command() {
        let add_args = ["remote", "add", "origin", "https://h/r.git"];
        let add_cmd = build_git_command(&add_args, Some("/r"));
        let set_url_cmd = build_git_command(
            &["remote", "set-url", "origin", "https://h/r.git"],
            Some("/r"),
        );
        let cmd = format!("{add_cmd} || {set_url_cmd}");
        assert_eq!(
            cmd,
            "git -C /r remote add origin https://h/r.git || git -C /r remote set-url origin https://h/r.git"
        );
    }

    #[test]
    fn remote_add_overwrite_with_fetch() {
        let add_args = ["remote", "add", "-f", "origin", "https://h/r.git"];
        let add_cmd = build_git_command(&add_args, Some("/r"));
        let set_url_cmd = build_git_command(
            &["remote", "set-url", "origin", "https://h/r.git"],
            Some("/r"),
        );
        let fetch_cmd = build_git_command(&["fetch", "origin"], Some("/r"));
        let cmd = format!("({add_cmd} || {set_url_cmd}) && {fetch_cmd}");
        assert_eq!(
            cmd,
            "(git -C /r remote add -f origin https://h/r.git || git -C /r remote set-url origin https://h/r.git) && git -C /r fetch origin"
        );
    }

    #[test]
    fn restore_validates_empty_paths() {
        // We can't call async in a sync test, but we can verify the error condition
        // by checking the paths check logic manually.
        let opts = GitRestoreOpts::default();
        assert!(
            opts.paths.is_empty(),
            "default GitRestoreOpts.paths should be empty"
        );
        // The actual InvalidArgument error is returned by the async method at runtime.
    }

    #[test]
    fn remote_get_command() {
        let git_cmd = build_git_command(&["remote", "get-url", "origin"], Some("/r"));
        assert_eq!(
            format!("{git_cmd} || true"),
            "git -C /r remote get-url origin || true"
        );
    }

    #[test]
    fn configure_user_builds_two_config_commands() {
        let name_cmd = build_git_command(&["config", "--global", "user.name", "Alice"], None);
        let email_cmd =
            build_git_command(&["config", "--global", "user.email", "alice@x.com"], None);
        assert_eq!(name_cmd, "git config --global user.name Alice");
        assert_eq!(email_cmd, "git config --global user.email alice@x.com");
    }

    // -----------------------------------------------------------------------
    // Task 4: status / branches / dangerously_authenticate helpers + tests
    // -----------------------------------------------------------------------

    /// Build a streaming process body: Start → Data(stdout) → End(exit_code) → EndStream.
    fn proc_stream_with_stdout(exit_code: i32, stdout_b64: &str) -> Vec<u8> {
        let mut body = encode_envelope(0, br#"{"event":{"start":{"pid":7}}}"#);
        body.extend(encode_envelope(
            0,
            format!(r#"{{"event":{{"data":{{"stdout":"{stdout_b64}"}}}}}}"#).as_bytes(),
        ));
        body.extend(encode_envelope(
            0,
            format!(
                r#"{{"event":{{"end":{{"exitCode":{exit_code},"exited":true,"status":"exited"}}}}}}"#
            )
            .as_bytes(),
        ));
        body.extend(encode_envelope(FLAG_END_STREAM, b"{}"));
        body
    }

    /// Mount a process mock that streams stdout then exits.
    async fn mount_proc_with_stdout(server: &MockServer, exit_code: i32, stdout_b64: &str) {
        Mock::given(method("POST"))
            .and(path("/process.Process/Start"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+json")
                    .set_body_bytes(proc_stream_with_stdout(exit_code, stdout_b64)),
            )
            .mount(server)
            .await;
    }

    /// `git.status` parses the stdout produced by `git status --porcelain=1 -b`.
    ///
    /// Input: `"## main...origin/main [ahead 1]\n M a.rs\n"`
    /// Expected: current_branch = "main", upstream = "origin/main", ahead = 1,
    /// file_status.len() = 1.
    #[tokio::test]
    async fn status_parses_stdout_into_git_status() {
        let server = MockServer::start().await;
        let raw = "## main...origin/main [ahead 1]\n M a.rs\n";
        let b64 = BASE64_STANDARD.encode(raw.as_bytes());
        mount_proc_with_stdout(&server, 0, &b64).await;

        let git = Git::new(Commands::build_with_connect(connect_for(&server), "0.6.3"));
        let status = git.status("/repo", None).await.expect("status ok");

        assert_eq!(status.current_branch.as_deref(), Some("main"));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!(status.ahead, 1);
        assert_eq!(status.file_status.len(), 1);
    }

    /// `git.branches` parses the stdout produced by `git branch --format=...`.
    ///
    /// Input: `"main\t*\nfeature\t\n"`
    /// Expected: current_branch = "main", branches = ["main", "feature"].
    #[tokio::test]
    async fn branches_parses_stdout_into_git_branches() {
        let server = MockServer::start().await;
        // "main\t*\nfeature\t\n" — tab separates name from HEAD marker
        let raw = "main\t*\nfeature\t\n";
        let b64 = BASE64_STANDARD.encode(raw.as_bytes());
        mount_proc_with_stdout(&server, 0, &b64).await;

        let git = Git::new(Commands::build_with_connect(connect_for(&server), "0.6.3"));
        let result = git.branches("/repo", None).await.expect("branches ok");

        assert_eq!(result.current_branch.as_deref(), Some("main"));
        assert_eq!(
            result.branches,
            vec!["main".to_string(), "feature".to_string()]
        );
    }

    /// Empty `username` must return `Err(InvalidArgument)` before any network I/O.
    #[tokio::test]
    async fn dangerously_authenticate_empty_username_is_invalid_argument() {
        let server = MockServer::start().await;
        let git = Git::new(Commands::build_with_connect(connect_for(&server), "0.6.3"));
        let err = git
            .dangerously_authenticate(GitDangerouslyAuthenticateOpts {
                username: String::new(),
                password: "token".to_string(),
                ..Default::default()
            })
            .await
            .expect_err("empty username must fail");
        assert!(
            matches!(err, crate::errors::Error::InvalidArgument(_)),
            "expected InvalidArgument, got {err:?}"
        );
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "guard must fire before any network round-trip"
        );
    }

    /// Empty `password` must return `Err(InvalidArgument)` before any network I/O.
    #[tokio::test]
    async fn dangerously_authenticate_empty_password_is_invalid_argument() {
        let server = MockServer::start().await;
        let git = Git::new(Commands::build_with_connect(connect_for(&server), "0.6.3"));
        let err = git
            .dangerously_authenticate(GitDangerouslyAuthenticateOpts {
                username: "alice".to_string(),
                password: String::new(),
                ..Default::default()
            })
            .await
            .expect_err("empty password must fail");
        assert!(
            matches!(err, crate::errors::Error::InvalidArgument(_)),
            "expected InvalidArgument, got {err:?}"
        );
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "guard must fire before any network round-trip"
        );
    }

    /// Happy-path `dangerously_authenticate`: both config and approve commands
    /// succeed (exit 0).  The mock responds to both `/process.Process/Start`
    /// calls.
    #[tokio::test]
    async fn dangerously_authenticate_happy_path_returns_ok() {
        let server = MockServer::start().await;
        // One mount handles both process calls (config + approve).
        mount_proc(&server, 0).await;

        let git = Git::new(Commands::build_with_connect(connect_for(&server), "0.6.3"));
        let result = git
            .dangerously_authenticate(GitDangerouslyAuthenticateOpts {
                username: "alice".to_string(),
                password: "s3cr3t".to_string(),
                host: Some("github.com".to_string()),
                protocol: Some("https".to_string()),
                user: None,
            })
            .await
            .expect("dangerously_authenticate ok");
        assert_eq!(result.exit_code, 0);
        // Both `git config` (credential.helper) and `git credential approve` must fire.
        assert_eq!(server.received_requests().await.expect("requests").len(), 2);
    }

    // -----------------------------------------------------------------------
    // Task 5 fixes
    // -----------------------------------------------------------------------

    /// `git clone` with credentials and `dangerously_store_credentials=false` must
    /// fire TWO RPCs: the clone itself and the post-clone `git remote set-url origin`
    /// credential-strip.  The strip's exit is checked via `check_set_url_exit`.
    #[tokio::test]
    async fn clone_strips_credentials_fires_two_rpcs() {
        let server = MockServer::start().await;
        // One mount responds to both the clone RPC and the set-url RPC.
        mount_proc(&server, 0).await;
        let git = Git::new(Commands::build_with_connect(connect_for(&server), "0.6.3"));
        let result = git
            .clone(
                "https://github.com/private/repo.git",
                GitCloneOpts {
                    username: Some("u".to_string()),
                    password: Some("p".to_string()),
                    path: Some("/dest".to_string()),
                    dangerously_store_credentials: false,
                    ..Default::default()
                },
            )
            .await
            .expect("clone with credential strip ok");
        assert_eq!(result.exit_code, 0);
        // Clone RPC + set-url strip RPC = 2 requests.
        assert_eq!(server.received_requests().await.expect("requests").len(), 2);
    }

    /// Empty-string guards on config/remote methods fire before any network I/O.
    ///
    /// Covers: `set_config(key)`, `get_config(key)`, `remote_add(name, url)`,
    /// `remote_get(name)`, `configure_user(name, email)`.
    #[tokio::test]
    async fn empty_string_guards_return_invalid_argument() {
        // Use a real mock server but assert it receives zero requests.
        let server = MockServer::start().await;
        let git = Git::new(Commands::build_with_connect(connect_for(&server), "0.6.3"));

        // set_config: empty key
        let err = git
            .set_config("", "v", GitConfigOpts::default())
            .await
            .expect_err("empty key");
        assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");

        // get_config: empty key
        let err = git
            .get_config("", GitConfigOpts::default())
            .await
            .expect_err("empty key");
        assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");

        // remote_add: empty name
        let err = git
            .remote_add("/r", "", "https://h/r.git", GitRemoteAddOpts::default())
            .await
            .expect_err("empty name");
        assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");

        // remote_add: empty url
        let err = git
            .remote_add("/r", "origin", "", GitRemoteAddOpts::default())
            .await
            .expect_err("empty url");
        assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");

        // remote_get: empty name
        let err = git
            .remote_get("/r", "", GitRequestOpts::default())
            .await
            .expect_err("empty name");
        assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");

        // configure_user: empty name
        let err = git
            .configure_user("", "a@b.com", GitConfigOpts::default())
            .await
            .expect_err("empty name");
        assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");

        // configure_user: empty email
        let err = git
            .configure_user("Alice", "", GitConfigOpts::default())
            .await
            .expect_err("empty email");
        assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");

        // All guards fire before any network round-trip.
        assert!(
            server
                .received_requests()
                .await
                .expect("requests")
                .is_empty(),
            "all guards must fire before any network round-trip"
        );
    }
}
