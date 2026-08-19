# Agent Feature Matrix

This document defines Rust-first acceptance criteria for the daemon’s agent, gateway, execution, persistence, and deployment capabilities. A feature is considered **implemented and tested** only when the repository contains an implementation, deterministic tests, and a documented operational boundary. External-service features are additionally marked as credential-dependent when they require live platform accounts.

| Capability | Rust extension point | Acceptance criteria | Status |
| --- | --- | --- | --- |
| Interactive terminal UI | `xai-grok-pager` | Multiline input, history, slash commands, streaming output, interrupt and redirect | Baseline upstream; daemon command parity remains |
| Headless agent runtime | Upstream shell and pager packages | Stable non-TUI entry point with structured events and resumable sessions | Implemented as the optional headless-agent bridge; live model credentials required |
| Tool execution | `xai-grok-tools` and workspace crates | File, shell, search, patch, browser, media, and approval-aware tools with bounded output | Upstream baseline; daemon routing and policy extension ongoing |
| E2B execution | `xai-grok-superagent::e2b` and vendored `third_party/e2b-rs` | Create, run, collect bounded output, enforce limits, and clean up sandboxes | Implemented and unit-tested; live execution requires `e2b` build feature and `E2B_API_KEY` |
| Provider routing | `xai-grok-superagent::provider` plus upstream model/config crates | Switch built-in providers and models without code changes; support validated custom OpenAI-compatible endpoints | Built-in profile registry, authenticated `/v1/providers`, and secret-free upstream TOML generation are implemented; full provider health/usage accounting and Anthropic runtime coverage remain partial |
| Persistent memory | `xai-grok-superagent::memory` | Restart-safe session-scoped entries, tag/text search, and bounded result count | Implemented and unit-tested through authenticated `/v1/memory`; typed entries, importance ranking, explicit deletion, compaction, statistics, export, and restore are implemented |
| Skills | `xai-grok-superagent::integrations` plus upstream skill crates | Discover, trust, invoke, and improve skills using an agentskills.io-compatible layout | Discovery registry implemented and unit-tested; invocation, trust policy, installation, and improvement loop remain |
| MCP | `xai-grok-superagent::integrations` plus upstream MCP crates | Configure stdio/HTTP/SSE servers, authenticate safely, enumerate tools, and isolate failures | Descriptor parsing plus bounded HTTP/SSE initialization, discovery, and authenticated `tools/call` execution are implemented with deterministic mocks; stdio runtime execution, richer trust policy, and failure isolation remain |
| Subagents | shell/session support | Spawn isolated parallel workers, cap depth and budget, collect results, and cancel safely | Bounded subagent execution is implemented with depth, concurrency, timeout, isolated session, and output limits; cancellation and richer provider-native orchestration remain |
| Scheduled automation | `xai-grok-superagent::scheduler` and `jobs` | Durable cron/interval jobs, retries, delivery routing, audit trail, and missed-run behavior | Durable interval schedules, background due-job claiming, leased job worker execution, bounded result persistence, crash recovery, retries, and target-based result delivery are implemented; cron expressions and richer audit/missed-run policy remain |
| Messaging channels | `xai-grok-superagent::gateway` plus gateway modules | Telegram, Discord, Slack, WhatsApp, Signal, Matrix, generic webhooks, email, pairing, rate limits, and cross-channel sessions | Telegram, Discord, Slack, WhatsApp, Signal, Matrix, generic authenticated webhooks, IMAP/SMTP email, and a target-based outbound dispatcher are implemented with deterministic tests; authenticated API rate limiting is implemented; richer inbound cross-channel session federation remains |
| Voice and media | provider registries and gateway clients | Transcription, TTS, image generation, video generation, and platform-native file delivery behind optional providers | Secure `MEDIA:` parsing plus Telegram document, Discord file, Slack upload, and WhatsApp native media delivery are implemented; voice/STT/TTS and generation providers remain |
| Browser/computer use | `xai-grok-superagent::browser` plus E2B template | Browser sessions, screenshots, navigation, approvals, sandboxed credentials, and a live user-visible Computer pane | E2B template foundation, Chromium/agent-browser stream supervision, app-compatible `/v1/computer/status`, and view-only `/v1/computer/browser` SSE frames are implemented; agent input actions and gateway-level user takeover controls remain |
| Security | config, approvals, hooks, workspace, gateway | Command approval, DM pairing, secrets scoping, sandbox isolation, redaction, and emergency stop | Core approval policy, deny-by-default gateway authorization, pairing, E2B isolation, non-root deployment, bearer-token auth, protected request-body limits, security headers, and configurable per-session rate limits are implemented; redaction and emergency-stop parity remain |
| Observability | diagnostics and daemon status | Structured logs, run traces, health/readiness, usage accounting, gateway state, and crash recovery | Health, capability, connection readiness, daemon status, authenticated `/v1/observability`, durable leases/recovery, and gateway retry logging are implemented; structured traces and usage accounting remain |
| 24/7 service operation | headless binary plus Docker/systemd assets | Restart-safe service, durable queue/leases, graceful shutdown, health checks, backups, and upgrade/rollback | Docker Compose, systemd, non-root runtime, health checks, durable jobs/memory/schedules/controls/cursors, background worker recovery, automatic gateway startup, runtime backups, and health-checked upgrade/rollback scripts are implemented |
| Research workflows | batch runner and trajectory crates | Batch trajectories, compression, replay, dataset export, and deterministic test fixtures | Upstream baseline; daemon workflow integration remains |

## Current verified quality gates

The current repository passes the following commands with Rust **1.94.0**:

```bash
cargo +1.94.0 fmt --all -- --check
cargo +1.94.0 clippy -p xai-grok-superagent --features e2b -- -D warnings
cargo +1.94.0 test -p xai-grok-superagent --features e2b
```

The feature-gated superagent suite currently reports **45 library tests plus 2 daemon tests passing**; email, bounded MCP execution, and target-based delivery each have deterministic coverage.
 Gateway tests are deterministic normalization/authentication tests; they do not claim live platform connectivity without the corresponding credentials and service accounts.

The final release claim distinguishes among **implemented and tested**, **implemented but requiring external service credentials**, **partial foundation**, **planned**, and **not supported**. This avoids presenting a robust daemon foundation as complete feature parity.
