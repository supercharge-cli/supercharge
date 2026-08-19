# Hermes capability parity target

This document translates the publicly stated Hermes Agent capability set into Rust-first acceptance criteria for the supercharged grok-build implementation. A feature is not considered complete until it has an implementation owner, configuration surface, tests, and a documented failure mode.

| Capability | Rust extension point | Acceptance criteria | Status |
| --- | --- | --- | --- |
| Interactive terminal UI | `xai-grok-pager` | Multiline input, history, slash commands, streaming output, interrupt and redirect | Baseline; extend |
| Headless agent runtime | `xai-grok-shell` | Stable non-TUI entry point with structured events and resumable sessions | Baseline; extend |
| Tool execution | `xai-grok-tools` and workspace crates | File, shell, search, patch, browser, media, and approval-aware tools with bounded output | Baseline; extend |
| E2B execution | workspace/execution backend | Create, connect, renew, pause/resume where supported, collect logs, enforce timeouts, and clean up sandboxes | Planned |
| Provider routing | model/config crates | Switch providers and models without code changes; preserve streaming and tool-call semantics | Baseline; extend |
| Persistent memory | memory crates | User profile, durable facts, best practices, FTS/session search, compaction, and explicit deletion | Baseline; extend |
| Skills | agent prompt/skills and plugin crates | Discover, install, trust, invoke, and improve skills using an agentskills.io-compatible layout | Baseline; extend |
| MCP | MCP crate | Configure stdio/HTTP/SSE servers, authenticate safely, enumerate tools, and isolate failures | Baseline; extend |
| Subagents | shell/session support | Spawn isolated parallel workers, cap depth and budget, collect results, and cancel safely | Planned |
| Scheduled automation | new scheduler service or adapted hooks | Durable cron jobs, retries, delivery routing, audit trail, and missed-run behavior | Planned |
| Messaging channels | gateway transport layer | Telegram, Discord, Slack, WhatsApp, Signal, email, pairing, rate limits, and cross-channel sessions | Planned |
| Voice and media | provider registries | Transcription, TTS, image generation, and video generation behind optional providers | Planned |
| Browser/computer use | browser/tool backend | Browser sessions, screenshots, navigation, approvals, and sandboxed credentials | Planned |
| Security | config, hooks, workspace, gateway | Command approval, DM pairing, secrets scoping, sandbox isolation, redaction, and emergency stop | Baseline; harden |
| Observability | diagnostics and new metrics layer | Structured logs, run traces, health/readiness, usage accounting, and crash recovery | Planned |
| 24/7 service operation | headless binary plus deployment assets | Restart-safe service, durable queue/leases, graceful shutdown, health checks, backups, and upgrade/rollback | Planned |
| Research workflows | batch runner and trajectory crates | Batch trajectories, compression, replay, dataset export, and deterministic test fixtures | Baseline; extend |

## Non-negotiable quality gates

A milestone may be pushed only when `cargo fmt --all -- --check`, targeted `cargo check`, targeted unit tests, and relevant integration tests pass. Features requiring external credentials must have deterministic mock tests and an explicit live-test procedure; lack of credentials must never be reported as a passing live integration test.

The final release claim will distinguish among **implemented and tested**, **implemented but requiring external service credentials**, **planned**, and **not supported**. This avoids presenting a prototype as Hermes parity.
