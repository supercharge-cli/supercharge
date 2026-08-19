# Supercharged Grok Build Agent

This private repository is the Rust-first implementation of the agent requested by the project owner: a substantially extended agent built on the public `xai-org/grok-build` workspace, with Hermes-style capabilities added around the same runtime rather than a separate Python service.

The current tree is based on upstream grok-build revision `c2dab005c2b53c99209dc6faf82153d782f76ee4`. The upstream Rust workspace already provides the core agent shell, tools, workspace/execution boundaries, configuration, MCP support, memory-related crates, hooks, plugins, headless operation, and terminal UI. The work in this private repository will extend those boundaries instead of replacing them with a second runtime.

## Target product

The target is an over-the-top, self-hostable coding and general-purpose agent with a Rust core, isolated E2B execution, provider-neutral model routing, persistent memory, skills, MCP, subagents, scheduled jobs, approvals, observability, and messaging/channel adapters. The feature inventory and acceptance criteria are tracked in [`docs/hermes-parity.md`](docs/hermes-parity.md).

## Current implementation status

The repository has now been reset from the earlier Python proof of concept to the actual Rust workspace baseline. That Python prototype was not the requested final product and is no longer part of the implementation tree. The next milestones will add the supercharged runtime capabilities in Rust, with tests and a pushed commit at each verified stage.

This is an active engineering build, not a claim that full Hermes parity is already complete. Features are marked as **baseline**, **in progress**, **planned**, or **blocked by an external credential/service** rather than being represented as finished before they are tested.

## Build baseline

The upstream instructions require the pinned Rust toolchain, DotSlash, and `protoc`. The primary binary is assembled by `crates/codegen/xai-grok-pager-bin`; the agent runtime and headless entry points live in `crates/codegen/xai-grok-shell`; tools and workspace execution are separated into their own crates. Use targeted Cargo checks first because the full workspace is large.

```bash
cargo check -p xai-grok-pager-bin
cargo test -p xai-grok-config
cargo fmt --all -- --check
```

## Licensing and attribution

The upstream first-party source in this tree is distributed under the Apache License, Version 2.0, with third-party notices retained in `THIRD-PARTY-NOTICES` and component-specific notices. See [`NOTICE-UPSTREAM.md`](NOTICE-UPSTREAM.md). Hermes Agent is used as a feature reference; no Hermes source files are being copied into this baseline unless a future implementation explicitly records the applicable license and attribution.

## Production direction

The Rust service will be packaged for a persistent host with restart supervision and durable state. E2B is the isolated execution backend, not the permanent host for the agent process. Production credentials, channel tokens, model keys, and user data must remain outside Git.
