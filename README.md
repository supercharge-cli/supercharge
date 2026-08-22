# Supercharged Rust Agent

This private repository is a Rust-first, self-hostable agent daemon with durable execution, messaging gateways, memory, jobs, approvals, and isolated cloud execution. It extends a public upstream Rust agent workspace rather than maintaining a separate Python service.

The current tree is based on a pinned upstream Rust workspace revision. That workspace provides the core agent shell, tools, workspace/execution boundaries, configuration, MCP support, memory-related crates, hooks, plugins, headless operation, and terminal UI. This repository extends those boundaries instead of replacing them with a second runtime.

## Target product

The target is an over-the-top, self-hostable coding and general-purpose agent with a Rust core, isolated E2B execution, provider-neutral model routing, persistent memory, skills, MCP, subagents, scheduled jobs, approvals, observability, and messaging/channel adapters. The feature inventory and acceptance criteria are tracked in [`docs/feature-parity.md`](docs/feature-parity.md).

## Current implementation status

The repository has now been reset from the earlier Python proof of concept to the actual Rust workspace baseline. That Python prototype was not the requested final product and is no longer part of the implementation tree. The next milestones will add the supercharged runtime capabilities in Rust, with tests and a pushed commit at each verified stage.

This is an active engineering build, not a claim that every planned feature is already complete. Features are marked as **baseline**, **in progress**, **planned**, or **blocked by an external credential/service** rather than being represented as finished before they are tested.

## Build baseline

The upstream instructions require the pinned Rust toolchain, DotSlash, and `protoc`. The primary binary is assembled by the upstream pager package; the agent runtime and headless entry points live in the upstream shell package, while tools and workspace execution are separated into their own crates. Use targeted Cargo checks first because the full workspace is large.

```bash
cargo check -p supercharge-pager-bin
cargo test -p supercharge-config
cargo fmt --all -- --check
```

## Licensing and attribution

The upstream first-party source in this tree is distributed under the Apache License, Version 2.0, with third-party notices retained in `THIRD-PARTY-NOTICES` and component-specific notices. See [`NOTICE-UPSTREAM.md`](NOTICE-UPSTREAM.md). This project is independently branded and does not present itself as an upstream product.

## Production direction

The Rust service will be packaged for a persistent host with restart supervision and durable state. E2B is the isolated execution backend, not the permanent host for the agent process. Production credentials, channel tokens, model keys, and user data must remain outside Git.
