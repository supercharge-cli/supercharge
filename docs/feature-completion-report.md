# Rust-First Agent Daemon Platform: Feature-Completion Report

**Repository:** `iotserver24/hermes-agent-alternative`  
**Companion repository:** `iotserver24/cloud-infra`  
**Report status:** Final implementation snapshot  
**Agent repository commit:** `50db4d6` — `feat: add health-checked upgrade rollback tooling`  
**Cloud-infra commit:** `099c6ab` — `fix: sync Rust agent inbox credentials`

## Executive summary

The project now provides a private, Rust-first, self-hostable 24/7 agent daemon built on the existing Rust workspace. The core runtime, persistence, multi-channel gateway layer, E2B execution lifecycle, provider configuration, browser workspace stream, security middleware, automation stores, email gateway, bounded MCP HTTP/SSE execution, and operational upgrade/rollback tooling are implemented and validated.

The main unfinished publication item is the second interactive E2B image. The Rust daemon binary is already staged in the template source, but the upstream provider-bridge executable could not be produced reliably in this sandbox: repeated builds were terminated by the environment during the final compile/link stage, including stripped and lower-memory linker attempts. No placeholder bridge binary was introduced, so the repository does not claim a falsely publishable image.

## Implemented capabilities

| Area | Current implementation status |
| --- | --- |
| Rust daemon mode | Long-running Axum service with graceful shutdown, background scheduler, durable job worker, automatic gateway startup, health endpoint, and daemon status endpoint. |
| Messaging gateways | Telegram long polling and webhook mode, Discord, Slack, WhatsApp, Signal, Matrix, generic authenticated webhooks, and allowlisted IMAP/SMTP email. |
| Durable state | Restart-safe jobs, leases, retries, memory, schedules, session controls, gateway offsets, Matrix cursor, and E2B session metadata. |
| Memory | Typed entries, tags, importance ranking, session-scoped search, deletion, compaction, statistics, export, and restore. |
| E2B | One-shot execution plus durable sessions with bounded workspace paths, capped output, auto-pause/auto-resume lifecycle, wake, pause, and deletion routes. |
| Browser workspace | Chromium and `agent-browser` stream supervision source, `/v1/computer/status`, and app-compatible view-only browser SSE frames. |
| Providers | Twelve built-in provider profiles, custom OpenAI-compatible endpoint validation, secret-free TOML generation, and authenticated provider configuration API. |
| MCP | Controlled descriptor parsing, HTTP/SSE initialization and tool discovery, plus authenticated bounded `tools/call` execution with session-header propagation, argument limits, timeout limits, and JSON-RPC error handling. |
| Subagents | Bounded depth, concurrency, timeout, isolated session, and output limits. |
| Media | Secure `MEDIA:` parsing and native delivery for Telegram, Discord, Slack, and WhatsApp. |
| Security | Bearer-token API protection, deny-by-default gateway authorization, request-size limits, security headers, E2B workspace validation, rate limits, non-root deployment assets, and approval policy. |
| Operations | Docker/systemd assets, runtime backup/restore scripts, health-checked atomic upgrade, automatic failed-upgrade recovery, and explicit rollback tooling. |
| Cloud compatibility | `cloud-infra` supports configurable daemon commands and emits neutral `SUPERAGENT_*` runtime variables for the Rust template while preserving the legacy template path. |

## Validation results

The final Rust quality gates were run with Rust **1.94.0**, the E2B feature enabled, offline dependency resolution, and a lower-memory linker configuration suitable for this sandbox.

| Check | Result |
| --- | --- |
| `cargo +1.94.0 fmt --all -- --check` | Passed |
| `cargo +1.94.0 clippy --offline --locked -p xai-grok-superagent --features e2b -- -D warnings` | Passed |
| `cargo +1.94.0 test --offline --locked -p xai-grok-superagent --features e2b` | Passed: **44 library tests, 2 daemon tests** |
| Upgrade and rollback shell syntax | Passed with `bash -n` |
| Git working trees | Clean in both repositories |
| Agent repository push | `50db4d6` is on `origin/main` |
| Cloud-infra push | `099c6ab` is on `origin/main` |

The 44 library tests include deterministic email parsing/allowlist coverage and an HTTP-mocked MCP initialization, session propagation, and tool-call test. The tests do not claim live connectivity to external platforms without corresponding credentials and service accounts.

## Interactive E2B template status

The template source is located at `sandbox/e2b-agent-template/`. It contains the Dockerfile, entrypoint, wake service, browser stream dependencies, neutral environment contract, and the staged Rust daemon binary. The intended image exposes the wake/file-share service on port `8788`, the Rust daemon API on port `8790`, and the browser stream on port `9223`.

The provider bridge remains the publication blocker. The build was retried with single-job compilation, reduced features, stripped debug information, isolated target directories, GNU gold, and GCC-driven LLVM lld. The environment repeatedly terminated the final upstream build process with SIGTERM before producing a usable provider-bridge executable. The E2B API key was not written to Git, and no fake or placeholder executable was staged.

## Deployment instructions

For a persistent Linux host, copy `.env.runtime.example` to a protected secret file, set `SUPERAGENT_API_TOKEN`, provider credentials, and only the gateway variables that are intentionally enabled. Persist `/var/lib/superagent` or the configured runtime directory. The systemd unit is `deploy/superagent.service`; it runs as a non-root `superagent` user and restarts the daemon after crashes.

For E2B execution, build the daemon with the `e2b` feature and provide `E2B_API_KEY`. Use the durable session routes when a filesystem and sandbox should survive across commands. The next SDK/API operation wakes a paused sandbox; auto-resume is activity-driven rather than an unbounded background process.

For email, set `EMAIL_USERNAME`, `EMAIL_PASSWORD`, and a non-empty `EMAIL_ALLOWED_SENDERS` list. Configure IMAP and SMTP host, port, and TLS variables. The adapter uses the host certificate store, polls unread mail, derives sessions from sender and subject, marks accepted messages as seen, and sends plain-text replies through authenticated SMTP. Use an application password or provider-issued credential and keep it outside Git and runtime backups.

For controlled MCP execution, set `SUPERAGENT_MCP_SERVERS` to a JSON descriptor list. Use authenticated `GET /v1/integrations/mcp/tools` for discovery and authenticated `POST /v1/integrations/mcp/call` for HTTP/SSE tool invocation. The call route enforces a 256 KiB argument limit and a 1–120 second timeout bound. Stdio MCP execution is intentionally not exposed through the HTTP route.

For binary upgrades, run:

```bash
deploy/upgrade.sh /path/to/xai-grok-superagent-server
```

The script creates a pre-upgrade runtime backup, stages a release, atomically replaces the configured binary, restarts an active systemd service, and requires `/healthz` to succeed. A failed health check restores the previous executable. To explicitly roll back the latest release, run:

```bash
deploy/rollback.sh
```

Use `SUPERAGENT_INSTALL_ROOT`, `SUPERAGENT_BINARY_PATH`, `SUPERAGENT_SERVICE_NAME`, and `SUPERAGENT_HEALTH_URL` when the host layout differs from the defaults.

## Remaining work

The remaining items are advanced parity and product-expansion work rather than missing daemon foundations. Cross-channel message routing and scheduled-result delivery still need a unified outbound dispatcher. Voice, speech-to-text, text-to-speech, image generation, and video-generation provider adapters are not yet implemented. Full stdio MCP execution, richer skill trust and invocation policy, structured traces, usage accounting, redaction, emergency-stop parity, provider health checks, cron-expression scheduling, and a repository CI workflow remain future work. The GitHub token available to the session does not have permission to push workflow files.

The interactive E2B image must still be published after a reliable provider-bridge binary is obtained. Once that binary is available, run the template preparation script, publish `sandbox/e2b-agent-template/`, record the returned immutable template identifier in `e2b.toml`, update `docs/feature-parity.md`, and push the resulting commit.

## Repository hygiene

No API keys, mailbox credentials, E2B credentials, or provider secrets were committed. The two repositories are private and clean at the reported commits. Runtime secrets remain environment- or secret-manager-owned, and the deployment documentation explicitly treats durable backups as sensitive operational data.
