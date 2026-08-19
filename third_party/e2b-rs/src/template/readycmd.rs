//! Ready-check command generators for [`super::ReadyCmd`].
//!
//! Each free function returns a [`ReadyCmd`] whose [`ReadyCmd::cmd`] method
//! yields the exact POSIX shell command string that a sandbox agent should
//! run to decide whether the sandbox is ready to serve traffic. The strings
//! are ported verbatim from the JavaScript SDK's `readycmd.ts`.

use crate::utils::shell_quote;

/// An opaque wrapper around a POSIX shell command string used as a ready-check.
///
/// Instances are produced by the free functions in this module
/// ([`wait_for_port`], [`wait_for_url`], [`wait_for_process`],
/// [`wait_for_file`], [`wait_for_timeout`]). Use [`cmd`][ReadyCmd::cmd] to
/// inspect the generated command.
#[derive(Debug, Clone)]
pub struct ReadyCmd {
    cmd: String,
}

impl ReadyCmd {
    /// Returns the generated shell command string.
    pub fn cmd(&self) -> &str {
        &self.cmd
    }

    /// Consumes `self` and returns the inner command string.
    ///
    /// Used internally by the template builder (Plan 5c) to serialize the
    /// ready-check command into the sandbox build request.
    // Plan 5c will call this; suppress dead_code until that task lands.
    #[allow(dead_code)]
    pub(crate) fn into_cmd(self) -> String {
        self.cmd
    }
}

/// Wait for a TCP port to be listening.
///
/// Uses `ss` to check for an exact source-port match so that, for example,
/// port 80 does not accidentally match port 8080. `ss` exits 0 regardless of
/// whether any sockets matched, so the command tests for non-empty output.
///
/// # Example
///
/// ```
/// use e2b_rs::template::wait_for_port;
///
/// let cmd = wait_for_port(8080);
/// assert_eq!(cmd.cmd(), r#"[ -n "$(ss -Htuln sport = :8080)" ]"#);
/// ```
pub fn wait_for_port(port: u16) -> ReadyCmd {
    ReadyCmd {
        cmd: format!(r#"[ -n "$(ss -Htuln sport = :{port})" ]"#),
    }
}

/// Wait for a URL to return a specific HTTP status code.
///
/// Uses `curl` to perform a silent request, captures only the numeric HTTP
/// status code, and pipes it to `grep -q` for an exact match.
///
/// The conventional default status code in the JavaScript SDK is **200**.
/// Pass `200` explicitly or supply a different code when the service uses a
/// non-standard success response (e.g. `201`, `204`).
///
/// # Example
///
/// ```
/// use e2b_rs::template::wait_for_url;
///
/// let cmd = wait_for_url("http://localhost:3000/health", 200);
/// assert_eq!(
///     cmd.cmd(),
///     r#"curl -s -o /dev/null -w "%{http_code}" http://localhost:3000/health | grep -q "200""#
/// );
/// ```
pub fn wait_for_url(url: &str, status_code: u16) -> ReadyCmd {
    ReadyCmd {
        cmd: format!(
            r#"curl -s -o /dev/null -w "%{{http_code}}" {} | grep -q "{}""#,
            shell_quote(url),
            status_code,
        ),
    }
}

/// Wait for a process with the given name to be running.
///
/// Uses `pgrep` to search for a live process whose name matches `name`.
/// Output is discarded; only the exit code matters.
///
/// # Example
///
/// ```
/// use e2b_rs::template::wait_for_process;
///
/// let cmd = wait_for_process("node");
/// assert_eq!(cmd.cmd(), "pgrep node > /dev/null");
/// ```
pub fn wait_for_process(name: &str) -> ReadyCmd {
    ReadyCmd {
        cmd: format!("pgrep {} > /dev/null", shell_quote(name)),
    }
}

/// Wait for a file at `path` to exist.
///
/// Uses the POSIX shell `[ -f … ]` test, so the file must be a regular
/// file (not a directory or symlink to a non-regular file).
///
/// # Example
///
/// ```
/// use e2b_rs::template::wait_for_file;
///
/// let cmd = wait_for_file("/tmp/ready");
/// assert_eq!(cmd.cmd(), "[ -f /tmp/ready ]");
/// ```
pub fn wait_for_file(path: &str) -> ReadyCmd {
    ReadyCmd {
        cmd: format!("[ -f {} ]", shell_quote(path)),
    }
}

/// Wait for a fixed duration before considering the sandbox ready.
///
/// `timeout_ms` is converted to whole seconds with integer floor division;
/// the minimum sleep duration is 1 second regardless of how small `timeout_ms`
/// is. This matches the JavaScript SDK's `Math.max(1, Math.floor(timeout / 1000))`.
///
/// # Example
///
/// ```
/// use e2b_rs::template::wait_for_timeout;
///
/// assert_eq!(wait_for_timeout(5000).cmd(), "sleep 5");
/// assert_eq!(wait_for_timeout(500).cmd(),  "sleep 1"); // minimum 1 second
/// ```
pub fn wait_for_timeout(timeout_ms: u64) -> ReadyCmd {
    let seconds = (timeout_ms / 1000).max(1);
    ReadyCmd {
        cmd: format!("sleep {seconds}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_cmd_strings_match_js() {
        assert_eq!(
            wait_for_port(8080).cmd(),
            r#"[ -n "$(ss -Htuln sport = :8080)" ]"#
        );
        assert_eq!(wait_for_process("node").cmd(), "pgrep node > /dev/null");
        assert_eq!(wait_for_file("/tmp/ready").cmd(), "[ -f /tmp/ready ]");
        assert_eq!(wait_for_timeout(5000).cmd(), "sleep 5");
        assert_eq!(wait_for_timeout(500).cmd(), "sleep 1"); // min 1s
        let u = wait_for_url("http://localhost:3000", 200);
        assert_eq!(
            u.cmd(),
            r#"curl -s -o /dev/null -w "%{http_code}" http://localhost:3000 | grep -q "200""#
        );
    }

    #[test]
    fn ready_cmd_shell_quotes_args() {
        // a path with a space must be shell-quoted by wait_for_file
        assert_eq!(wait_for_file("/tmp/a b").cmd(), "[ -f '/tmp/a b' ]");
    }
}
