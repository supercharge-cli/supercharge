# Supercharge — Comprehensive Agent Task List

This document is self-contained. All file paths are relative to the workspace root (`/home/r3ap3reditz/codes/supercharge` or wherever this repo is cloned). Do NOT reference paths outside the repo.

---

## TASK 1: Rename xAI/Grok Branding → Supercharge

The codebase was forked from an xAI Grok project. All user-visible branding, CLI flag names, crate names, and internal identifiers must be renamed to Supercharge so no xAI/Grok origin is visible.

### 1a. Broken CLI flags (critical — runtime crash)

File: `crates/codegen/xai-grok-shell/src/leader/mod.rs`

Lines ~1695-1697 have broken flags from a bad find-and-replace:
```rust
cmd.arg("--Supercharge AI-ws-url")    // BROKEN: space in flag name
cmd.arg("--Supercharge AI-ws-origin") // BROKEN: space in flag name
```
These must be:
```rust
cmd.arg("--supercharge-ws-url")
cmd.arg("--supercharge-ws-origin")
```

Then find ALL places that parse these flags (grep for `ws-url`, `ws-origin`, `grok-ws-url`, `grok-ws-origin`, `Supercharge AI-ws`) and update them consistently.

### 1b. Dead if/else (code smell)

File: `crates/codegen/xai-grok-shell/src/managed_config.rs` ~line 466

```rust
let env_name = if std::env::var_os("SUPERCHARGE_DEPLOYMENT_KEY").is_some() {
    "SUPERCHARGE_DEPLOYMENT_KEY"
} else {
    "SUPERCHARGE_DEPLOYMENT_KEY"  // same in both branches — dead conditional
};
```
Simplify to just `let env_name = "SUPERCHARGE_DEPLOYMENT_KEY";`

### 1c. Crate-level renames

These crates still have `xai-grok-*` in their names. Rename them to `supercharge-*`:
- `xai-grok-pager` → `supercharge-pager`
- `xai-grok-pager-bin` → `supercharge-pager-bin`
- `xai-grok-shell` → `supercharge-shell`
- `xai-grok-sampler` → `supercharge-sampler`
- `xai-grok-sampling-types` → `supercharge-sampling-types`
- `xai-grok-agent` → `supercharge-agent`
- `xai-grok-telemetry` → `supercharge-telemetry`
- `xai-grok-update` → `supercharge-update`
- `xai-grok-mcp` → `supercharge-mcp`
- `xai-grok-memory` → `supercharge-memory`
- `xai-grok-hooks` → `supercharge-hooks`
- `xai-grok-config` → `supercharge-config`
- `xai-grok-config-types` → `supercharge-config-types`
- `xai-grok-shared` → `supercharge-shared`
- `xai-grok-home` → `supercharge-home`
- `xai-grok-tools` → `supercharge-tools`
- `xai-grok-http` → `supercharge-http`
- `xai-grok-sandbox` → `supercharge-sandbox`
- `xai-grok-secrets` → `supercharge-secrets`
- `xai-grok-workspace` → `supercharge-workspace`
- `xai-grok-workspace-types` → `supercharge-workspace-types`
- `xai-grok-workspace-client` → `supercharge-workspace-client`
- `xai-grok-workspace-daemon` → `supercharge-workspace-daemon`
- `xai-grok-models` → `supercharge-models`
- `xai-grok-bundle` → `supercharge-bundle`
- `xai-grok-voice` → `supercharge-voice`
- `xai-grok-mermaid` → `supercharge-mermaid`
- `xai-grok-markdown` → `supercharge-markdown`
- `xai-grok-markdown-core` → `supercharge-markdown-core`
- `xai-grok-plugin-marketplace` → `supercharge-plugin-marketplace`
- `xai-grok-session-search` → `supercharge-session-search`
- `xai-grok-session-events` → `supercharge-session-events`
- `xai-grok-foreign-sessions` → `supercharge-foreign-sessions`
- `xai-grok-active-sessions` → `supercharge-active-sessions`
- `xai-grok-pager-diff` → `supercharge-pager-diff`
- `xai-grok-pager-render` → `supercharge-pager-render`
- `xai-grok-pager-minimal` → `supercharge-pager-minimal`
- `xai-grok-announcements` → `supercharge-announcements`
- `xai-grok-auth` → `supercharge-auth`
- `xai-grok-env` → `supercharge-env`
- `xai-grok-paths` → `supercharge-paths`
- `xai-grok-extra-ca` → `supercharge-extra-ca`
- `xai-grok-version` → `supercharge-version`
- `xai-grok-shell-base` → `supercharge-shell-base`
- `xai-grok-shell-session-support` → `supercharge-shell-session-support`
- `xai-grok-subagent-resolution` → `supercharge-subagent-resolution`
- `xai-grok-superagent` → `supercharge-superagent`
- `xai-grok-test-support` → `supercharge-test-support`
- `xai-grok-diag-server` → `supercharge-diag-server`
- `xai-grok-gix-status` → `supercharge-gix-status`
- `xai-grok-foreign-sessions` → `supercharge-foreign-sessions`
- `xai-grok-hunk-tracker` → `supercharge-hunk-tracker`
- `xai-acp-lib` → `supercharge-acp-lib`
- `xai-chat-state` → `supercharge-chat-state`
- `xai-compaction-transcript` → `supercharge-compaction-transcript`
- `xai-crash-handler` → `supercharge-crash-handler`
- `xai-fast-worktree` → `supercharge-fast-worktree`
- `xai-file-utils` → `supercharge-file-utils`
- `xai-fsnotify` → `supercharge-fsnotify`
- `xai-fuzzy-file-search` → `supercharge-fuzzy-file-search`
- `xai-agent-lifecycle` → `supercharge-agent-lifecycle`
- `xai-mixpanel` → `supercharge-mixpanel`
- `xai-prompt-queue` → `supercharge-prompt-queue`
- `xai-sqlite-journal` → `supercharge-sqlite-journal`
- `xai-system-power` → `supercharge-system-power`
- `xai-token-estimation` → `supercharge-token-estimation`
- `xai-tracing-macros` → `supercharge-tracing-macros`
- `xai-tty-utils` → `supercharge-tty-utils`
- `xai-workflow` → `supercharge-workflow`
- `xai-hooks-plugins-types` → `supercharge-hooks-plugins-types`
- `xai-codebase-graph` → `supercharge-codebase-graph`

**For each rename:**
1. Rename the directory under `crates/codegen/`
2. Update `Cargo.toml` in the crate (change `[package] name`)
3. Update `Cargo.toml` workspace root `members` list
4. Update ALL `use xai_grok_*` / `use xai_*` imports across the entire workspace
5. Update ALL `[dependencies]` in every `Cargo.toml` that references the old name
6. Run `cargo check --ignore-rust-version -j 4` to verify compilation

### 1d. User-visible strings

Grep the entire workspace for these patterns and replace:
- `grok login` → `supercharge login`
- `grok logout` → `supercharge logout`
- `grok` (as a CLI command reference) → `supercharge`
- `x.ai` (as a domain reference in user-facing strings) → keep if it's an actual API endpoint, replace if it's branding
- `Grok` (product name in UI strings) → `Supercharge`
- `xAI` (company name in UI strings) → appropriate replacement

### 1e. Config paths

Check if `~/.grok/` is used as a config directory. If so, rename to `~/.supercharge/`. Key files:
- `crates/codegen/xai-grok-home/src/` — home directory resolution
- `crates/codegen/xai-grok-paths/src/` — path constants
- `crates/codegen/xai-grok-shell/src/agent/config.rs` — config loading

---

## TASK 2: Fix Routing.run / Deepseek Streaming Issue

The routing.run proxy returns broken streaming responses for deepseek-v4-flash. Streaming chunks contain only `"tool_calls": []` with no content. Non-streaming works fine.

### 2a. Add `function_call` to `ChatChunkDelta`

File: `crates/codegen/xai-grok-sampling-types/src/types.rs` ~line 636

The `ChatChunkDelta` struct is missing a `function_call` field. Deepseek sends `"function_call": {"name": "", "arguments": ""}` which could cause deserialization issues.

```rust
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ChatChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    pub reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty", deserialize_with = "deserialize_null_default")]
    pub tool_calls: Vec<ToolCallDelta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    // ADD THIS:
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<serde_json::Value>,
}
```

Also add to `ChatResponseMessage` (~line 492):
```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub function_call: Option<serde_json::Value>,
```

### 2b. Non-streaming fallback for empty streaming responses

File: `crates/codegen/xai-grok-sampler/src/actor/request_task.rs`

In `run_one_attempt` (~line 520), after `stream_chat_completions` returns, check if the response has empty content with reasoning. If so, retry with a non-streaming call.

Alternatively, add a model-level flag `disable_streaming: bool` to `ModelInfo` (in `crates/codegen/xai-grok-shell/src/agent/config.rs` ~line 4152) that forces non-streaming for specific models.

### 2c. Test

After fixes, run:
```bash
curl -s https://api.routing.run/v1/chat/completions \
  -H "Authorization: Bearer YOUR_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"deepseek-v4-flash","messages":[{"role":"user","content":"say hello"}],"max_tokens":100,"stream":true}'
```
Verify chunks contain `content` or `reasoning_content` fields (not just empty `tool_calls`).

---

## TASK 3: Animated ASCII Logo

### Current state
File: `crates/codegen/xai-grok-pager/src/views/welcome/logo.rs`

The logo uses braille art from `crates/codegen/xai-grok-pager/assets/logo/logo07.txt` (full) and `logo05.txt` (small). It has a shimmer animation via `shimmer_frame()` at ~12fps.

### What to do
Replace the static braille logo with an animated ASCII art logo that:
1. Uses only ASCII characters (no braille Unicode) for maximum terminal compatibility
2. Has multiple animation frames (at least 4-8 frames)
3. Shows a "lightning bolt" or "power up" animation effect
4. The animation should play on startup and then settle to a static frame

Files to modify:
- `crates/codegen/xai-grok-pager/assets/logo/` — add new ASCII frame files
- `crates/codegen/xai-grok-pager/src/views/welcome/logo.rs` — update to cycle frames
- `crates/codegen/xai-grok-pager/src/views/welcome/hero_box.rs` — adjust sizing if needed

---

## TASK 4: Daemon Mode (Long-running Agent)

Implement a daemon mode where the agent runs continuously in the background, accessible via Telegram, Discord, or a REST API.

### 4a. Architecture

The daemon should:
1. Run as a background process (systemd service or similar)
2. Accept commands from multiple interfaces (Telegram bot, Discord bot, HTTP API)
3. Maintain persistent sessions
4. Handle multiple concurrent conversations
5. Auto-reconnect on failures

### 4b. Key files to modify/create

- `crates/codegen/xai-grok-superagent/` — the existing superagent server (Axum-based)
- `crates/codegen/xai-grok-shell/src/agent/app.rs` — agent lifecycle
- New crate: `crates/codegen/supercharge-daemon/` for the daemon entry point

### 4c. Telegram integration

Create a Telegram bot handler:
- Use `teloxide` or `telera` crate
- Map Telegram messages to agent prompts
- Stream agent responses back to Telegram
- Handle file attachments (upload to agent workspace)

### 4d. Discord integration

Create a Discord bot handler:
- Use `serenity` or `poise` crate
- Slash commands for /ask, /setup, /status
- Channel-based session management

### 4e. HTTP API

Extend the existing Axum server with:
- `POST /api/v1/chat` — send a message, get a response
- `GET /api/v1/sessions` — list active sessions
- `POST /api/v1/sessions` — create a new session
- `WebSocket /api/v1/stream` — stream responses in real-time

---

## TASK 5: Security Testing

Add comprehensive security tests that, when all green, prove the codebase is secure.

### 5a. Credential handling tests

File locations to test:
- `crates/codegen/xai-grok-shell/src/agent/config.rs` — config loading (API keys)
- `crates/codegen/xai-grok-shell/src/agent/provider_setup.rs` — provider setup
- `crates/codegen/xai-grok-shell/src/agent/auth_method.rs` — auth methods

Tests needed:
1. API keys are never logged (check tracing macros don't include key values)
2. API keys are redacted in error messages
3. Config file permissions are restrictive (0600)
4. Keyring/secret storage uses platform-native secure storage
5. No API keys in process environment visible to child processes

### 5b. Input validation tests

- SQL injection in session names/queries
- Path traversal in file operations (workspace paths, config paths)
- Command injection in shell execution
- XSS in any HTML-rendered output
- SSRF in URL handling (API endpoints, MCP servers)

### 5c. Authentication tests

- Token expiry handling
- Session fixation prevention
- CSRF protection on any web endpoints
- Rate limiting on auth attempts

### 5d. Data isolation tests

- Sessions don't leak data between users
- Temporary files are cleaned up
- Memory is zeroed after use for sensitive data
- Logs don't contain conversation content

### 5e. Dependency audit

Run `cargo audit` and fix any known vulnerabilities.

---

## TASK 6: Comprehensive Test Suite

### 6a. Unit tests for new features

For the `/setup` modal changes:
- File: `crates/codegen/xai-grok-pager/src/views/setup_modal.rs`
- Test: modal state transitions (empty → filling → submitting → done/error)
- Test: input validation (empty API key, invalid URL, model selection)

For the provider setup:
- File: `crates/codegen/xai-grok-shell/src/agent/provider_setup.rs`
- Test: `persist_provider_setup` writes correct TOML
- Test: `fetch_models` handles network errors gracefully
- Test: model list deduplication

For the BYOK preference:
- File: `crates/codegen/xai-grok-shell/src/agent/models.rs`
- Test: `sampling_config` prefers BYOK model when current has no credentials
- Test: `sampling_config` falls back correctly when no BYOK models exist

For FinishReason tolerance:
- File: `crates/codegen/xai-grok-sampling-types/src/types.rs`
- Test: empty string deserializes to `FinishReason::Unknown`
- Test: known variants still deserialize correctly

### 6b. Integration tests

- Full setup flow: open modal → enter API key → fetch models → select → save → verify model switched
- Send message with custom provider → verify correct endpoint called
- Retry logic: verify empty response triggers retry, max retries reached shows error

### 6c. Run all tests

```bash
cargo test --ignore-rust-version -j 4 2>&1 | tee test-results.txt
```

Known pre-existing failures (DO NOT FIX):
- `crates/codegen/xai-fast-worktree/src/api/gc/integration_tests.rs:1022` — u64 vs usize comparison
- `crates/codegen/xai-grok-pager/tests/registered_features_are_documented.rs` — missing doc files

---

## Execution Order

1. **Task 1c+1d first** (crate renames) — everything else depends on clean names
2. **Task 1a** (broken flags) — critical runtime fix
3. **Task 1b** (dead code) — trivial cleanup
4. **Task 2** (streaming fix) — functional fix
5. **Task 6** (tests) — verify everything works
6. **Task 3** (ASCII animation) — cosmetic
7. **Task 5** (security tests) — thoroughness
8. **Task 4** (daemon mode) — new feature, largest scope

## Verification

After ALL tasks, run:
```bash
cargo check --ignore-rust-version -j 4 --workspace
cargo test --ignore-rust-version -j 4 2>&1 | grep -E "^test result|FAILED|error\["
cargo clippy --ignore-rust-version -j 4 --workspace 2>&1 | grep -E "^error|warning\["
```

All three must pass (except known pre-existing failures).
