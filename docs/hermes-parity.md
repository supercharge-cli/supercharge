# Hermes-style parity matrix

This document translates the publicly stated Hermes Agent capability set into Rust-first acceptance criteria for the grok-build-based superagent. A feature is considered **implemented and tested** only when the repository contains an implementation, deterministic tests, and a documented operational boundary. External-service features are additionally marked as credential-dependent when they require live platform accounts.

| Capability | Rust extension point | Acceptance criteria | Status |
| --- | --- | --- | --- |
| Interactive terminal UI | `xai-grok-pager` | Multiline input, history, slash commands, streaming output, interrupt and redirect | Baseline upstream; daemon command parity remains |
| Headless agent runtime | `xai-grok-shell` and `xai-grok-pager-bin` | Stable non-TUI entry point with structured events and resumable sessions | Implemented as the optional `GROK_AGENT_BINARY` bridge; live model credentials required |
| Tool execution | `xai-grok-tools` and workspace crates | File, shell, search, patch, browser, media, and approval-aware tools with bounded output | Upstream baseline; daemon routing and policy extension ongoing |
| E2B execution | `xai-grok-superagent::e2b` and vendored `third_party/e2b-rs` | Create, run, collect bounded output, enforce limits, and clean up sandboxes | Implemented and unit-tested; live execution requires `e2b` build feature and `E2B_API_KEY` |
| Provider routing | model/config crates | Switch providers and models without code changes while preserving tool-call semantics | Upstream baseline; daemon `/model` parity remains |
| Persistent memory | `xai-grok-superagent::memory` | Restart-safe session-scoped entries, tag/text search, and bounded result count | Implemented and unit-tested through authenticated `/v1/memory`; compaction and explicit deletion remain |
| Skills | `xai-grok-superagent::integrations` plus upstream skill crates | Discover, trust, invoke, and improve skills using an agentskills.io-compatible layout | Discovery registry implemented and unit-tested; invocation, trust policy, installation, and improvement loop remain |
| MCP | `xai-grok-superagent::integrations` plus upstream MCP crates | Configure stdio/HTTP/SSE servers, authenticate safely, enumerate tools, and isolate failures | Descriptor parsing/configuration foundation implemented; full runtime transport and tool execution remain |
| Subagents | shell/session support | Spawn isolated parallel workers, cap depth and budget, collect results, and cancel safely | Planned; capability switch exists |
| Scheduled automation | `xai-grok-superagent::scheduler` and `jobs` | Durable cron/interval jobs, retries, delivery routing, audit trail, and missed-run behavior | Interval model and durable job primitives implemented and tested; daemon tick, cron persistence, and home-channel delivery remain |
| Messaging channels | `xai-grok-superagent::gateway` plus gateway modules | Telegram, Discord, Slack, WhatsApp, Signal, Matrix, generic webhooks, email, pairing, rate limits, and cross-channel sessions | Telegram, Discord, Slack, WhatsApp, Signal, Matrix, and generic authenticated webhooks implemented with deterministic tests; email, rate limiting, media delivery, and cross-channel routing remain |
| Voice and media | provider registries and gateway clients | Transcription, TTS, image generation, video generation, and platform-native file delivery behind optional providers | Planned; media capability flags exist |
| Browser/computer use | browser/tool backend | Browser sessions, screenshots, navigation, approvals, and sandboxed credentials | Upstream capability surface; daemon-specific routing and credential boundary remain |
| Security | config, approvals, hooks, workspace, gateway | Command approval, DM pairing, secrets scoping, sandbox isolation, redaction, and emergency stop | Core approval policy, deny-by-default gateway authorization, pairing, E2B isolation, non-root deployment, and token auth implemented; rate limits, redaction, and emergency-stop parity remain |
| Observability | diagnostics and daemon status | Structured logs, run traces, health/readiness, usage accounting, gateway state, and crash recovery | Health, capability, daemon status, durable leases/recovery, and gateway retry logging implemented; traces and usage accounting remain |
| 24/7 service operation | headless binary plus Docker/systemd assets | Restart-safe service, durable queue/leases, graceful shutdown, health checks, backups, and upgrade/rollback | Docker Compose, systemd, non-root runtime, health checks, durable jobs/memory/cursors, and automatic gateway startup implemented; backup/restore and upgrade rollback remain |
| Research workflows | batch runner and trajectory crates | Batch trajectories, compression, replay, dataset export, and deterministic test fixtures | Upstream baseline; daemon workflow integration remains |

## Current verified quality gates

The current repository passes the following commands with Rust **1.94.0**:

```bash
cargo +1.94.0 fmt --all -- --check
cargo +1.94.0 clippy -p xai-grok-superagent --features e2b -- -D warnings
cargo +1.94.0 test -p xai-grok-superagent --features e2b
```

The feature-gated superagent suite currently reports **23 passing tests**. Gateway tests are deterministic normalization/authentication tests; they do not claim live platform connectivity without the corresponding credentials and service accounts.

The final release claim distinguishes among **implemented and tested**, **implemented but requiring external service credentials**, **partial foundation**, **planned**, and **not supported**. This avoids presenting a robust daemon foundation as complete Hermes feature parity.
