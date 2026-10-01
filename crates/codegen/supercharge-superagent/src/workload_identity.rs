use std::process::Command as StdCommand;

const DEFAULT_UID: u32 = 1000;
const DEFAULT_GID: u32 = 1000;

fn configured_id(name: &str, fallback: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(fallback)
}

pub fn uid() -> u32 {
    configured_id("SUPERAGENT_WORKLOAD_UID", DEFAULT_UID)
}

pub fn gid() -> u32 {
    configured_id("SUPERAGENT_WORKLOAD_GID", DEFAULT_GID)
}

#[cfg(unix)]
fn should_drop() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
fn should_drop() -> bool {
    false
}

/// Drop supplementary groups, then set GID/UID before exec. std's `groups()`
/// is still unstable, so this uses the same primitives libc exposes.
#[cfg(unix)]
fn drop_identity_preexec(uid: u32, gid: u32) -> impl FnMut() -> std::io::Result<()> {
    move || {
        if unsafe { libc::setgroups(0, std::ptr::null()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if unsafe { libc::setgid(gid) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if unsafe { libc::setuid(uid) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

pub fn apply_std(command: &mut StdCommand) {
    apply_std_as(command, uid(), gid());
}

pub fn apply_std_as(command: &mut StdCommand, uid: u32, gid: u32) {
    #[cfg(unix)]
    if should_drop() {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(drop_identity_preexec(uid, gid));
        }
        command
            .env("HOME", "/home/user")
            .env("USER", "user")
            .env("LOGNAME", "user");
    }
}

pub fn apply_tokio(command: &mut tokio::process::Command) {
    apply_tokio_as(command, uid(), gid());
}

pub fn apply_tokio_as(command: &mut tokio::process::Command, uid: u32, gid: u32) {
    #[cfg(unix)]
    if should_drop() {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.as_std_mut().pre_exec(drop_identity_preexec(uid, gid));
        }
        command
            .env("HOME", "/home/user")
            .env("USER", "user")
            .env("LOGNAME", "user");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_sandbox_user() {
        if std::env::var_os("SUPERAGENT_WORKLOAD_UID").is_none() {
            assert_eq!(uid(), 1000);
        }
        if std::env::var_os("SUPERAGENT_WORKLOAD_GID").is_none() {
            assert_eq!(gid(), 1000);
        }
    }
}
