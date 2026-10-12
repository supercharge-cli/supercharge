# Supercharge Upstream Feature Parity Implementation Plan

> For agentic workers: implement one independently testable task at a time. Use test-first changes, review each task, and preserve unrelated worktree edits.

**Goal:** Add the eight retained capabilities identified in upstream `2bdd1d6a6` to Supercharge without importing xAI/Grok/Grok Build names, defaults, URLs, or user-facing copy. Per-folder website restrictions were withdrawn at the user's request.

**Architecture:** Adapt behavior to existing `supercharge-*` crates rather than wholesale replacing files. Keep CLI presentation, configuration, memory persistence, and the lifecycle broker as separate change sets.

**Reference:** `upstream/main` at `2bdd1d6a6` (fetched once); local baseline `b96c73f9d`. These histories have no merge base. Local theme edits and `supercharge-superagent/src/live.rs` are unrelated and must remain intact.

**Constraints:** No release, tag, push, or local installation requested. Use Supercharge paths, crate names, environment names, log targets, and copy. Avoid literal x.ai/Grok identifiers except protocol keys required for interoperability. Keep Rust builds under the previously requested 15 GB ceiling when building a release; use limited jobs for tests.

## Phase 1 — CLI and presentation

### Task 1: Selectable model context window
- Reference: `upstream/main:crates/codegen/xai-grok-pager/src/slash/commands/context_window.rs`, `slash/commands/model.rs`, `acp/model_state.rs`, `app/actions.rs`.
- Local: `crates/codegen/supercharge-pager/src/{acp/model_state.rs,slash/commands/model.rs,slash/commands/mod.rs,app/actions.rs}` and corresponding model-switch dispatch. Add a session-scoped `/context-window` command and model picker options; preserve effort selection. Validate choices advertised by the active model and carry the selected nonzero token count to the shell rather than changing display only.
- Test first: invalid size leaves state unchanged; supported size survives switch and reflects effective sampling/compaction window; model with one/no option hides the command. Run focused pager model command and model-state tests.

### Task 2: Honest headless MCP status
- Reference: `upstream/main:crates/codegen/xai-grok-pager/src/headless/mcp_init.rs`.
- Local: `crates/codegen/supercharge-pager/src/headless.rs` and a bounded resolver module. Query actual MCP status for the streaming `init` line, mapping connected, pending, needs-auth, disabled, and failed; timeouts must not claim connected. Do not expose secrets in output.
- Test first: configured but disconnected/failed servers never say connected; connected and auth-required states map correctly; timeout is bounded. Run focused pager headless tests.

### Task 3: Tracked TUI memory operations
- Reference: `upstream/main:crates/codegen/xai-grok-pager/src/slash/commands/{memory.rs,memory_ops.rs}`.
- Local: pager slash registry, dispatch, memory modal and shell memory builtins. Register `/memory`, `/flush`, `/dream` when supported by the shell. Queue `/flush` and `/dream` as tracked commands with progress and typed outcome; keep existing shell operation semantics and modal compatibility.
- Test first: visibility follows advertised builtins; invalid args are rejected; operation displays start/completion/failure rather than a silent prompt turn. Run pager slash and dispatch tests.

### Task 4: Dashboard preview toggle
- Reference: `upstream/main:crates/codegen/xai-grok-pager/src/settings/defs.rs`, `views/dashboard/preview.rs`.
- Local: pager settings definitions, dispatch persistence, dashboard preview layout. Persist an independent `dashboard_preview` preference; disabling it prevents preview work, while leaving the dashboard usable.
- Test first: enabled/disabled layouts, persistence/reload, no preview computation while off. Preserve current uncommitted theme settings edits when integrating.

### Task 5: Graceful headless signals
- Reference: `upstream/main:crates/codegen/xai-grok-pager/src/headless/signals.rs` and headless turn loop.
- Local: pager headless turn loop and binary entry point. Install handlers before one-shot agent startup; first signal cancels and performs bounded cleanup, repeated signal/timeout exits promptly, and no TUI escape sequence reaches stdout.
- Test first: signal arbitration and cleanup ordering with injected signal source; subprocess signal smoke test on Unix. Run focused headless tests.

## Phase 2 — Configuration and memory

### Task 6: Rotating MCP bearer-token files
- Reference: `upstream/main:crates/codegen/xai-grok-config/src/mcp_server_config.rs`, `xai-grok-mcp/src/{bearer_token_file.rs,mcp_http_client.rs}`.
- Local: `supercharge-config`, `supercharge-config-types`, and `supercharge-mcp`. Accept a path in HTTP MCP configuration, validate size/format and precedence against headers/OAuth, read token at request time for rotation, and redact it from diagnostics. Reject unsafe/empty file rather than falling back silently to a stale credential.
- Test first: rotation between requests, invalid and oversized files, redaction, precedence, and compatibility with existing OAuth. Run config and MCP tests.

### Task 7: Batch Dream
- Reference: `upstream/main:crates/codegen/xai-grok-memory/src/batch_dream*`, `xai-grok-shell/src/session/batch_dream.rs`.
- Local: `supercharge-memory`, `supercharge-config-types/src/memory.rs`, and shell Dream runner. Add bounded queue/leases, durable batch claims, idempotent commits/recovery, and configurable controls while preserving existing manual/automatic Dream behavior. Do not mark inputs processed before model output and durable writes succeed.
- Test first: cap, claim contention, interruption/retry, changed inputs, failure rollback, and successful reindex. Run memory and shell Dream tests.

## Phase 3 — Workspace runtime

### Task 8: Pre-stop lifecycle broker
- Reference: `upstream/main:crates/codegen/xai-grok-lifecycle/src`, `xai-grok-workspace/src/bin/workspace_server.rs`.
- Local: new `supercharge-lifecycle` crate, workspace server, diagnostics routing, and deployment hooks. Mount authenticated or local-socket-only pre-stop and handler routes; support bounded registrations, one active trigger, absolute deadline, capped logs and process-group termination. Coordinate with existing workspace drain instead of creating a second shutdown race. Avoid exposing privileged handler execution on a public listener.
- Test first: concurrent trigger conflict, deadline, persistence across process restart within a boot, invalid registration, route access, and shutdown integration. Run lifecycle and workspace tests.

## Integration gates

1. Review touched files against the original worktree and upstream reference; ensure no unwanted xAI/Grok/Grok Build branding in new Supercharge-owned identifiers or copy.
2. Run targeted tests after each task, then `cargo check --locked` for pager bin, shell, memory, MCP, workspace and new crates with constrained jobs. Run the relevant integration tests.
3. Run `git diff --check`, format only touched files, inspect the final diff for unrelated changes, and report any unverified platform paths plainly. Do not publish or install unless requested.
