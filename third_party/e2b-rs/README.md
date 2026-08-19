# e2b-rs

Rust SDK for [E2B](https://e2b.dev) — cloud sandboxes for AI agents. A 1:1 port
of the official [JavaScript SDK](https://github.com/e2b-dev/e2b/tree/main/packages/js-sdk),
built to feel familiar while reading as idiomatic async Rust.

> **Status:** feature-complete 1:1 port of the E2B JavaScript SDK. All
> subsystems are implemented: sandbox lifecycle, filesystem, commands, PTY,
> git, volumes, and the full template build pipeline (builder methods, file
> context upload, log streaming, tag management). MCP server wiring and the
> devcontainer-beta APIs are deferred by explicit decision; see
> `docs/parity-checklist.md` for details.

## Installation

```toml
[dependencies]
e2b-rs = "0.1"
```

## Quickstart

```rust
use e2b_rs::Sandbox;

let sandbox = Sandbox::create().template("base").await?;
println!("{}", sandbox.get_host(3000));
sandbox.kill().await?;
```

The filesystem API lets you read, write, and watch files inside the sandbox:

```rust
use e2b_rs::Sandbox;

let sandbox = Sandbox::create().template("base").await?;
sandbox.files().write("/tmp/hello.txt", b"hi".to_vec(), Default::default()).await?;
let text = sandbox.files().read("/tmp/hello.txt", None).await?;
let mut watch = sandbox.files().watch_dir("/tmp", Default::default()).await?;
while let Some(event) = watch.next().await {
    println!("{:?} {}", event.r#type, event.name);
}
```

Run commands inside the sandbox and stream their output:

```rust
use e2b_rs::{CommandOutput, Sandbox};

let sandbox = Sandbox::create().template("base").await?;
// Foreground: run to completion.
let result = sandbox.commands().run("echo hello", Default::default()).await?;
println!("exit {}: {}", result.exit_code, result.stdout);

// Background: stream output as it arrives.
let mut cmd = sandbox.commands().start("sleep 1; echo done", Default::default()).await?;
while let Some(out) = cmd.next().await {
    if let CommandOutput::Stdout(bytes) = out {
        print!("{}", String::from_utf8_lossy(&bytes));
    }
}
let _ = cmd.wait().await?;
```

Use the git API to clone repositories and inspect state inside the sandbox:

```rust
use e2b_rs::Sandbox;

let sandbox = Sandbox::create().template("base").await?;
sandbox.git().clone("https://github.com/owner/repo.git", Default::default()).await?;
let status = sandbox.git().status("/home/user/repo", None).await?;
println!("branch: {:?}", status.current_branch);
```

Use persistent volumes to share data between sandboxes or store large datasets
without bundling them into a template:

```rust
use e2b_rs::Volume;

let volume = Volume::create("my-data", Default::default()).await?;
volume.write_file("/hello.txt", b"hi".to_vec(), Default::default()).await?;
let text = volume.read_file("/hello.txt").await?;
println!("{text}");
```

Build a custom sandbox template using the fluent builder chain and stream the
build logs as they arrive. Convenience entry points cover common base images
(`from_python_image`, `from_node_image`, `from_bun_image`, `from_debian_image`,
`from_ubuntu_image`) as well as private registries (`from_aws_registry`,
`from_gcp_registry`). File-system and command builder methods (`copy`,
`run_cmd`, `set_workdir`, package installers, `git_clone`, …) produce
individual image layers:

```rust
use e2b_rs::{Template, wait_for_timeout};

// copy() returns Result<Template>, so we break the chain and propagate the error.
let template = Template::new().from_python_image("3.12");
let template = template.copy("requirements.txt", "/app/", Default::default())?;
let template = template
    .run_cmd("pip install -r /app/requirements.txt", Default::default())
    .set_workdir("/app")
    .set_start_cmd("python app.py", wait_for_timeout(20_000));

let mut build = template.build("my-app", Default::default()).await?;
while let Some(log) = build.next().await {
    println!("{log}");
}
let info = build.wait().await?;
println!("built template {}", info.template_id);
```

Control-plane extras are also available: pause/resume, metrics, snapshots
(create/list/delete), network-rule updates, and signed upload/download URLs.

```rust
use e2b_rs::Sandbox;

let sandbox = Sandbox::create().template("base").await?;
let metrics = sandbox.get_metrics().await?;
let snap = sandbox.create_snapshot(Some("nightly".to_string())).await?;
sandbox.pause().await?;
```

The library is imported as `e2b_rs`:

```rust
use e2b_rs::{ConnectionConfig, ConnectionConfigOpts};
```

## Design

- **Async-only** on `tokio`.
- **Channels, not callbacks:** streaming output (commands, pty, watch, build
  logs) is delivered through `tokio::sync::mpsc` receivers.
- **Panic-free library code:** `unwrap`/`expect` are denied outside tests.
- **MSRV:** Rust 1.95.0, edition 2024.

See `.super/specs/` for the full design and `docs/parity-checklist.md` for the
JS-to-Rust parity matrix.

## License

MIT
