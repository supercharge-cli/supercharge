# Supercharge desktop

First-party **ACP client** for Supercharge. It does not reimplement the agent.

```
supercharge-desktop
        │  JSON-RPC / stdio
        ▼
supercharge agent stdio    ← existing harness
```

## Run

```bash
cargo build -p supercharge-pager-bin
cargo run -p supercharge-desktop -- --bin target/debug/supercharge-pager --cwd .
```

Or put `supercharge` / `supercharge-pager` on `PATH`, or set `SUPERCHARGE_BIN`.

Login still happens in the agent (`supercharge login`). This window is chat, streaming, and permission cards only.
