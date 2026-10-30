use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{BwrapBackend, bwrap_args, find_bwrap, probe_bwrap};
use crate::command::backend::{
    BackendName, CallId, CommandTag, OriginalArgv, RenderedPolicy, SandboxBackend,
};
use crate::command::policy::{DenyEntry, EnvPolicy, NetworkPolicy, ReadPolicy, SandboxPolicy};
use crate::command::protected::Protected;

fn policy_for(write_roots: Vec<PathBuf>, protected: Vec<Protected>) -> SandboxPolicy {
    SandboxPolicy {
        read: ReadPolicy::AllExcept { deny: Vec::new() },
        write_roots,
        network: NetworkPolicy::Off,
        env: EnvPolicy::default(),
        protected,
        build_cache_trees: Vec::new(),
        unread_git_metadata: Vec::new(),
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "supercharge-bwrap-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dunce::canonicalize(dir).unwrap()
}

fn argv(program: &str, args: &[&str], cwd: &Path) -> OriginalArgv {
    OriginalArgv {
        program: PathBuf::from(program),
        args: args.iter().map(OsString::from).collect(),
        cwd: cwd.to_path_buf(),
    }
}

fn strings(args: &[OsString]) -> Vec<String> {
    args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
}

/// The index of the first `flag a b` triple, for ordering asserts.
fn position(args: &[String], window: &[&str]) -> usize {
    args.windows(window.len())
        .position(|w| w.iter().zip(window).all(|(a, b)| a == b))
        .unwrap_or_else(|| panic!("{window:?} not in {args:?}"))
}

#[test]
fn mandatory_entries_render_after_every_allow_and_the_command_comes_last() {
    let ws = scratch("order");
    let git = ws.join(".git");
    std::fs::create_dir_all(git.join("objects")).unwrap();
    let secret = ws.join("secret.txt");
    std::fs::write(&secret, "s").unwrap();
    let mut policy = policy_for(
        vec![ws.clone()],
        vec![Protected::TreeExcept {
            tree: git.clone(),
            except: git.join("objects"),
        }],
    );
    policy.read = ReadPolicy::AllExcept {
        deny: vec![DenyEntry::Path(secret.clone())],
    };
    let args = strings(&bwrap_args(&policy, &argv("/bin/echo", &["hi"], &ws)).unwrap());
    let ws_s = ws.to_string_lossy().into_owned();
    let git_s = git.to_string_lossy().into_owned();
    let objects_s = git.join("objects").to_string_lossy().into_owned();
    let secret_s = secret.to_string_lossy().into_owned();
    let read = position(&args, &["--ro-bind", "/", "/"]);
    let write = position(&args, &["--bind-try", &ws_s, &ws_s]);
    let tree = position(&args, &["--ro-bind", &git_s, &git_s]);
    let except = position(&args, &["--bind", &objects_s, &objects_s]);
    let deny = position(&args, &["--ro-bind", "/dev/null", &secret_s]);
    assert!(read < write && write < tree && tree < except && except < deny);
    assert!(args.contains(&"--unshare-net".to_owned()));
    assert_eq!(
        &args[args.len() - 3..],
        &["--".to_owned(), "/bin/echo".to_owned(), "hi".to_owned()]
    );
    std::fs::remove_dir_all(&ws).unwrap();
}

#[test]
fn the_proxy_network_keeps_the_host_network_and_relative_roots_are_refused() {
    let ws = scratch("net");
    let mut policy = policy_for(vec![ws.clone()], Vec::new());
    policy.network = NetworkPolicy::Proxy { port: 3128 };
    let args = strings(&bwrap_args(&policy, &argv("/bin/true", &[], &ws)).unwrap());
    assert!(!args.contains(&"--unshare-net".to_owned()));
    let relative = policy_for(vec![PathBuf::from("rel")], Vec::new());
    assert!(bwrap_args(&relative, &argv("/bin/true", &[], &ws)).is_err());
    std::fs::remove_dir_all(&ws).unwrap();
}

#[test]
fn protected_paths_outside_writable_roots_and_missing_ones_add_no_mounts() {
    let ws = scratch("skip");
    let outside = scratch("skip-outside");
    let policy = policy_for(
        vec![ws.clone()],
        vec![
            Protected::Path {
                path: outside.clone(),
            },
            Protected::Path {
                path: ws.join("missing"),
            },
        ],
    );
    let args = strings(&bwrap_args(&policy, &argv("/bin/true", &[], &ws)).unwrap());
    let outside_s = outside.to_string_lossy().into_owned();
    assert!(!args.contains(&outside_s));
    assert!(!args.iter().any(|a| a.ends_with("/missing")));
    std::fs::remove_dir_all(&ws).unwrap();
    std::fs::remove_dir_all(&outside).unwrap();
}

/// A working bubblewrap on this host, or `None` so the real-run tests are skipped.
fn real_backend() -> Option<BwrapBackend> {
    let path = find_bwrap()?;
    probe_bwrap(&path).then(|| BwrapBackend::new(path))
}

async fn run(
    backend: &BwrapBackend,
    policy: &SandboxPolicy,
    cwd: &Path,
    script: &str,
) -> std::process::Output {
    let original = argv("/bin/sh", &["-c", script], cwd);
    let mut cmd = tokio::process::Command::new(&original.program);
    cmd.args(&original.args).env("SC_BWRAP_PROBE", "carried");
    let receipt = backend
        .wrap(&mut cmd, &original, policy, &CommandTag::for_call(&CallId::parse("t1")))
        .unwrap();
    assert_eq!(BackendName::Bwrap, receipt.backend);
    assert!(matches!(receipt.rendered, RenderedPolicy::Bwrap { .. }));
    cmd.output().await.unwrap()
}

#[tokio::test]
async fn real_bwrap_confines_writes_protects_and_denies() {
    let Some(backend) = real_backend() else {
        eprintln!("skipped: no working bubblewrap on this host");
        return;
    };
    let ws = scratch("real");
    let outside = scratch("real-outside");
    let git = ws.join(".git");
    std::fs::create_dir_all(&git).unwrap();
    std::fs::write(git.join("config"), "[core]\n").unwrap();
    let secret = ws.join("secret.txt");
    std::fs::write(&secret, "token").unwrap();
    let mut policy = policy_for(
        vec![ws.clone()],
        vec![Protected::Path { path: git.clone() }],
    );
    policy.read = ReadPolicy::AllExcept {
        deny: vec![DenyEntry::Path(secret.clone())],
    };

    let ok = run(&backend, &policy, &ws, "echo in > inside.txt && echo $SC_BWRAP_PROBE").await;
    assert!(ok.status.success(), "{ok:?}");
    assert_eq!("carried\n", String::from_utf8_lossy(&ok.stdout));
    assert_eq!("in\n", std::fs::read_to_string(ws.join("inside.txt")).unwrap());

    let script = format!("echo x > {}/escaped.txt", outside.display());
    assert!(!run(&backend, &policy, &ws, &script).await.status.success());
    assert!(!outside.join("escaped.txt").exists());

    assert!(
        !run(&backend, &policy, &ws, "echo x >> .git/config")
            .await
            .status
            .success()
    );
    assert_eq!("[core]\n", std::fs::read_to_string(git.join("config")).unwrap());

    let read = run(&backend, &policy, &ws, "cat secret.txt").await;
    assert!(read.stdout.is_empty(), "{read:?}");

    std::fs::remove_dir_all(&ws).unwrap();
    std::fs::remove_dir_all(&outside).unwrap();
}

#[tokio::test]
async fn real_bwrap_without_network_has_only_loopback() {
    let Some(backend) = real_backend() else {
        eprintln!("skipped: no working bubblewrap on this host");
        return;
    };
    let ws = scratch("real-net");
    let policy = policy_for(vec![ws.clone()], Vec::new());
    let out = run(&backend, &policy, &ws, "cat /proc/net/dev").await;
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    let interfaces: Vec<&str> = text
        .lines()
        .skip(2)
        .filter_map(|line| line.split(':').next())
        .map(str::trim)
        .collect();
    assert_eq!(vec!["lo"], interfaces);
    std::fs::remove_dir_all(&ws).unwrap();
}
