#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use std::path::Path;

use crate::command::backend::BackendName;

use super::{BackendChoice, HostProbe, ProbedFile, SANDBOX_EXEC, detect_backend};

/// A probe that found `sandbox-exec` executable and applying the trivial profile.
fn working_probe() -> HostProbe {
    HostProbe {
        sandbox_exec_ok: true,
        sandbox_exec: ProbedFile::File { mode: 0o755 },
        ..HostProbe::default()
    }
}

#[test]
fn macos_needs_a_working_sandbox_exec() {
    assert_eq!(
        BackendChoice::Selected(BackendName::Seatbelt),
        working_probe().preferred_macos_backend()
    );
    assert!(matches!(
        HostProbe::default().preferred_macos_backend(),
        BackendChoice::Unavailable { .. }
    ));
}

/// The reason for "no backend" names what was checked and what was found — the path, what is at
/// it and its mode bits — so `sandbox.status` explains itself: a missing binary, a directory in
/// its place, a binary without an `x` bit, and one that is executable yet did not apply the
/// trivial profile all read differently.
#[test]
fn the_macos_reason_names_the_path_and_what_the_probe_found_there() {
    let reason = |probe: HostProbe| match probe.preferred_macos_backend() {
        BackendChoice::Unavailable { reason } => reason,
        BackendChoice::Selected(name) => panic!("selected {name:?} for {probe:?}"),
    };
    let cases: [(ProbedFile, &[&str]); 5] = [
        (
            ProbedFile::Missing {
                error: "No such file or directory (os error 2)".to_owned(),
            },
            &["not found", "No such file or directory"],
        ),
        (
            ProbedFile::NotAFile {
                kind: "directory".to_owned(),
            },
            &["a directory, not a regular file"],
        ),
        (
            ProbedFile::File { mode: 0o644 },
            &["mode 0644", "not executable"],
        ),
        (
            ProbedFile::File { mode: 0o755 },
            &[
                "mode 0755",
                "executable",
                "did not apply the trivial profile",
            ],
        ),
        (ProbedFile::Unchecked, &["not checked"]),
    ];
    for (found, expected) in cases {
        let text = reason(HostProbe {
            sandbox_exec_ok: false,
            sandbox_exec: found.clone(),
            ..HostProbe::default()
        });
        assert!(text.contains(SANDBOX_EXEC), "{found:?}: {text}");
        for fragment in expected {
            assert!(text.contains(fragment), "{found:?}: {fragment:?} in {text}");
        }
    }
}

/// Off macOS and Linux the reason names the OS and still says what was checked at the Seatbelt
/// path.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
#[test]
fn the_reason_off_macos_names_the_os_and_the_path_checked() {
    let BackendChoice::Unavailable { reason } = HostProbe {
        sandbox_exec_ok: false,
        sandbox_exec: ProbedFile::Missing {
            error: "No such file or directory (os error 2)".to_owned(),
        },
        ..HostProbe::default()
    }
    .preferred_backend() else {
        panic!("selected a backend off macOS");
    };
    for fragment in [
        std::env::consts::OS,
        SANDBOX_EXEC,
        "not found",
        "No such file or directory",
    ] {
        assert!(reason.contains(fragment), "{fragment:?} in {reason}");
    }
}

/// What the probe records for a path: `stat` through a symlink (the program `exec` would run),
/// the permission bits of a regular file, the kind of anything else, the OS text for nothing.
#[cfg(unix)]
#[test]
fn a_probed_file_records_the_mode_bits_the_kind_or_the_error() {
    use std::os::unix::fs::PermissionsExt;
    let root =
        std::env::temp_dir().join(format!("supercharge-sandbox-detect-probed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let binary = root.join("sandbox-exec");
    std::fs::write(&binary, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let plain = root.join("plain");
    std::fs::write(&plain, "").unwrap();
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
    let link = root.join("link");
    std::os::unix::fs::symlink(&binary, &link).unwrap();

    let executable = ProbedFile::read(&binary);
    assert_eq!(ProbedFile::File { mode: 0o755 }, executable);
    assert!(executable.is_executable());
    assert_eq!("mode 0755, executable", executable.to_string());
    assert_eq!(executable, ProbedFile::read(&link), "read through the link");
    let plain = ProbedFile::read(&plain);
    assert_eq!(ProbedFile::File { mode: 0o644 }, plain);
    assert!(!plain.is_executable());
    assert_eq!("mode 0644, not executable", plain.to_string());
    let dir = ProbedFile::read(&root);
    assert_eq!(
        ProbedFile::NotAFile {
            kind: "directory".to_owned()
        },
        dir
    );
    assert!(!dir.is_executable());
    let missing = ProbedFile::read(&root.join("missing"));
    assert!(
        matches!(&missing, ProbedFile::Missing { error } if error.contains("No such file")),
        "{missing:?}"
    );
    assert!(!missing.is_executable());
    assert!(!ProbedFile::Unchecked.is_executable());
    std::fs::remove_dir_all(&root).unwrap();
}

/// Off Linux: Seatbelt on macOS, and every other target selects nothing, whatever the probe
/// says, and `Enforce` refuses to run there.
#[test]
fn detection_selects_seatbelt_on_macos_and_nothing_elsewhere() {
    if cfg!(target_os = "linux") {
        return;
    }
    let probe = working_probe();
    let detected = detect_backend(&probe).map(|backend| backend.name());
    if cfg!(target_os = "macos") {
        assert_eq!(
            BackendChoice::Selected(BackendName::Seatbelt),
            probe.preferred_backend()
        );
        assert_eq!(Some(BackendName::Seatbelt), detected);
    } else {
        assert!(matches!(
            probe.preferred_backend(),
            BackendChoice::Unavailable { .. }
        ));
        assert_eq!(None, detected);
    }
    assert!(detect_backend(&HostProbe::default()).is_none());
}

/// The real probe on this machine: `sandbox-exec` applies the trivial profile, so the daemon
/// would select Seatbelt here.
#[cfg(target_os = "macos")]
#[test]
fn macos_host_probe_selects_seatbelt_from_the_real_sandbox_exec() {
    let probe = HostProbe::run();
    assert!(probe.sandbox_exec_ok);
    assert_eq!(
        BackendChoice::Selected(BackendName::Seatbelt),
        probe.preferred_backend()
    );
    assert_eq!(
        Some(BackendName::Seatbelt),
        detect_backend(&probe).map(|backend| backend.name())
    );
}

/// The real probe never selects a backend off macOS, whatever the host has installed, and it
/// records what is at the Seatbelt path rather than leaving it unchecked.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
#[test]
fn real_probe_selects_nothing_off_macos() {
    let probe = HostProbe::run();
    assert!(matches!(
        probe.preferred_backend(),
        BackendChoice::Unavailable { .. }
    ));
    assert!(detect_backend(&probe).is_none());
    assert_ne!(ProbedFile::Unchecked, probe.sandbox_exec);
    assert_eq!(
        ProbedFile::read(Path::new(SANDBOX_EXEC)),
        probe.sandbox_exec
    );
}

/// Linux selects bubblewrap only when it was found and created namespaces, and says why not.
#[test]
fn linux_selects_bwrap_when_it_was_found_and_works() {
    let found = |ok| HostProbe {
        bwrap: Some(std::path::PathBuf::from("/usr/bin/bwrap")),
        bwrap_ok: ok,
        ..HostProbe::default()
    };
    assert_eq!(
        BackendChoice::Selected(BackendName::Bwrap),
        found(true).preferred_linux_backend()
    );
    let BackendChoice::Unavailable { reason } = found(false).preferred_linux_backend() else {
        panic!("selected a backend that failed its probe");
    };
    assert!(reason.contains("/usr/bin/bwrap"), "{reason}");
    let BackendChoice::Unavailable { reason } = HostProbe::default().preferred_linux_backend()
    else {
        panic!("selected a backend that was not found");
    };
    assert!(reason.contains("not found"), "{reason}");
}

/// The real probe on a Linux host agrees with what is installed: a working bubblewrap is
/// selected and registered, otherwise nothing is.
#[cfg(target_os = "linux")]
#[test]
fn linux_real_probe_registers_bwrap_when_it_works() {
    let probe = HostProbe::run();
    let detected = detect_backend(&probe).map(|backend| backend.name());
    if probe.bwrap_ok {
        assert_eq!(Some(BackendName::Bwrap), detected);
    } else {
        assert_eq!(None, detected);
    }
}
