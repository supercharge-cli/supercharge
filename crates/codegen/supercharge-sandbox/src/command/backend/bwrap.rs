//! Linux per-command backend: the command runs under `bwrap` (bubblewrap) in fresh user and mount
//! namespaces. The rendered argv mounts the read view first, then the writable roots, then the
//! protected paths read-only and the read denies hidden, so nothing a grant adds can reopen a
//! mandatory deny (bubblewrap applies its mount operations in order).
//!
//! What bubblewrap cannot express is reported as `reduced`:
//! - protected paths that do not exist yet, glob entries and single symlink nodes are not mounted
//!   (a bind needs an existing source and follows links);
//! - glob read denies are not hidden;
//! - the proxy network mode keeps the host network, so only the proxy environment steers traffic.
//!   [`NetworkPolicy::Off`] drops the network entirely.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::command::backend::{
    BackendCapabilities, BackendName, CommandTag, OriginalArgv, RenderedPolicy, SandboxBackend,
    SandboxCommandError, WrapReceipt,
};
use crate::command::policy::{DenyEntry, NetworkPolicy, PolicyError, ReadPolicy, SandboxPolicy};
use crate::command::protected::Protected;

/// Where bubblewrap is looked for, by absolute path: never through `PATH`, which a workspace can
/// prepend to.
pub const BWRAP_CANDIDATES: &[&str] = &["/usr/bin/bwrap", "/bin/bwrap", "/usr/local/bin/bwrap"];

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// The first executable candidate, if any.
pub fn find_bwrap() -> Option<PathBuf> {
    BWRAP_CANDIDATES
        .iter()
        .map(PathBuf::from)
        .find(|path| super::super::detect::ProbedFile::read(path).is_executable())
}

/// Whether `bwrap` at `path` can create the namespaces a wrapped command needs on this host (a
/// kernel or security module may refuse unprivileged user namespaces).
pub fn probe_bwrap(path: &Path) -> bool {
    let mut cmd = std::process::Command::new(path);
    cmd.args([
        "--die-with-parent",
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--unshare-net",
        "--",
        "/bin/true",
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());
    supercharge_tty_utils::detach_std_command(&mut cmd);
    #[allow(clippy::disallowed_methods)] // enrolled via ProcessScope::enroll_std below
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    let _group = supercharge_tty_utils::global_process_scope()
        .enroll_std(&child)
        .ok();
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

pub struct BwrapBackend {
    program: PathBuf,
}

impl BwrapBackend {
    pub fn new(program: PathBuf) -> BwrapBackend {
        BwrapBackend { program }
    }
}

impl SandboxBackend for BwrapBackend {
    fn name(&self) -> BackendName {
        BackendName::Bwrap
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities { reduced: true }
    }

    fn wrap(
        &self,
        cmd: &mut tokio::process::Command,
        original: &OriginalArgv,
        policy: &SandboxPolicy,
        _tag: &CommandTag,
    ) -> Result<WrapReceipt, SandboxCommandError> {
        let args = bwrap_args(policy, original)?;
        let mut wrapper = tokio::process::Command::new(&self.program);
        wrapper.args(&args);
        for (name, value) in cmd.as_std().get_envs() {
            match value {
                Some(value) => {
                    wrapper.env(name, value);
                }
                None => {
                    wrapper.env_remove(name);
                }
            }
        }
        wrapper.current_dir(&original.cwd);
        let end = open_max_bound();
        // SAFETY: the hook calls only `fcntl` on integer descriptors: no allocation, no lock, so
        // it is safe between `fork` and `exec` in a multi-threaded parent.
        unsafe {
            wrapper.pre_exec(move || {
                mark_inherited_fds_cloexec(end);
                Ok(())
            });
        }
        *cmd = wrapper;
        Ok(WrapReceipt {
            backend: BackendName::Bwrap,
            rendered: RenderedPolicy::Bwrap {
                args: args
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect(),
            },
        })
    }
}

/// The bubblewrap argv for `policy` around `original`, pure so tests can assert it.
pub(crate) fn bwrap_args(
    policy: &SandboxPolicy,
    original: &OriginalArgv,
) -> Result<Vec<OsString>, SandboxCommandError> {
    let mut out = Args::default();
    out.flag("--die-with-parent");
    match &policy.read {
        ReadPolicy::AllExcept { .. } => out.pair("--ro-bind", Path::new("/")),
        ReadPolicy::Roots { roots, .. } => {
            for root in roots {
                require_absolute(root)?;
                out.pair("--ro-bind-try", root);
            }
        }
    }
    out.flag("--dev");
    out.path(Path::new("/dev"));
    out.flag("--proc");
    out.path(Path::new("/proc"));
    for root in &policy.write_roots {
        require_absolute(root)?;
        if is_symlinked(root) {
            return Err(SandboxCommandError::Unrenderable {
                backend: BackendName::Bwrap,
                reason: format!("writable root {} is reached through a symlink", root.display()),
            });
        }
        out.pair("--bind-try", root);
    }
    let writable = |path: &Path| policy.write_roots.iter().any(|root| path.starts_with(root));
    for protected in &policy.protected {
        match protected {
            Protected::Path { path } if writable(path) && exists_no_follow(path) => {
                if !is_symlink(path) {
                    out.pair("--ro-bind", path);
                }
            }
            Protected::TreeExcept { tree, except } if writable(tree) && exists_no_follow(tree) => {
                out.pair("--ro-bind", tree);
                if exists_no_follow(except) && writable(except) {
                    out.pair("--bind", except);
                }
            }
            _ => {}
        }
    }
    for deny in policy.read.deny() {
        if let DenyEntry::Path(path) = deny {
            match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.is_dir() => {
                    out.flag("--tmpfs");
                    out.path(path);
                    out.flag("--remount-ro");
                    out.path(path);
                }
                Ok(meta) if meta.is_file() => {
                    out.flag("--ro-bind");
                    out.path(Path::new("/dev/null"));
                    out.path(path);
                }
                _ => {}
            }
        }
    }
    if matches!(policy.network, NetworkPolicy::Off) {
        out.flag("--unshare-net");
    }
    out.flag("--chdir");
    out.path(&original.cwd);
    out.flag("--");
    out.0.push(original.program.as_os_str().to_owned());
    out.0.extend(original.args.iter().cloned());
    Ok(out.0)
}

#[derive(Default)]
struct Args(Vec<OsString>);

impl Args {
    fn flag(&mut self, flag: &str) {
        self.0.push(OsString::from(flag));
    }

    fn path(&mut self, path: &Path) {
        self.0.push(path.as_os_str().to_owned());
    }

    /// `flag src dest` with the same path for both.
    fn pair(&mut self, flag: &str, path: &Path) {
        self.flag(flag);
        self.path(path);
        self.path(path);
    }
}

fn require_absolute(path: &Path) -> Result<(), SandboxCommandError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(PolicyError::NotAbsolute {
            path: path.to_path_buf(),
        }
        .into())
    }
}

fn exists_no_follow(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// Whether an existing `path` resolves elsewhere: some component is a symlink.
fn is_symlinked(path: &Path) -> bool {
    dunce::canonicalize(path).is_ok_and(|real| real != path)
}

const FD_SWEEP_CEILING: libc::c_int = 65_536;

fn open_max_bound() -> libc::c_int {
    // SAFETY: `sysconf` takes no pointers.
    let open_max = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
    libc::c_int::try_from(open_max)
        .ok()
        .filter(|max| *max > 0)
        .map_or(FD_SWEEP_CEILING, |max| max.min(FD_SWEEP_CEILING))
}

/// Marks every descriptor above stderr close-on-exec, so only stdio reaches the sandbox.
fn mark_inherited_fds_cloexec(end: libc::c_int) {
    for fd in 3..end {
        // SAFETY: `fcntl` on an integer descriptor; a closed one returns EBADF.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }
}

#[cfg(test)]
#[path = "bwrap_tests.rs"]
mod tests;
