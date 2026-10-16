# ACP for Supercharge Studio

`supercharge agent serve` listens on `127.0.0.1:2419` by default (port 2420 is never bound by default) at
`ws://127.0.0.1:2419/ws?server-key=<key>`.

## Server key

The key is `--secret` / its env var if given, otherwise the persisted key file
`<SUPERCHARGE_HOME>/agent-serve.key` (created with mode 0600 on Unix).
`SUPERCHARGE_HOME` defaults to:

| OS | Default |
|---|---|
| Linux | `~/.supercharge` |
| macOS | `~/.supercharge` (`/Users/<you>/.supercharge`) |
| Windows | `%USERPROFILE%\.supercharge` |

A rejected upgrade returns `401` with `Content-Type: application/json`:

```json
{ "error": { "code": "server_key_missing" | "server_key_invalid", "message": "..." } }
```

On `server_key_invalid`, re-read the key file once and retry.

## Models

- `_supercharge/models/list` → `{ currentModelId, models: [{ id, displayName, provider, contextWindow, supportsReasoningEffort, pricing }] }`.
  Only models from the configured provider that have credentials are listed. `pricing` is `null` when unknown.
- Switch with `session/set_model`.
- A prompt on a model that is not configured fails with JSON-RPC error `-32050`,
  `data: { code: "model_not_configured", requested, configured: [ids] }`. There is no fallback model.

## @ mentions

Send each file or folder as `{ "type": "resource_link", "uri": "file:///abs/path", "name": "..." }` without `_meta`.
The CLI percent-decodes the URI (including `file:///C:/...` on Windows) and expands it like a typed `@path`
(paths with spaces become `@"..."`). Embedded `resource` blocks are also accepted and inlined as sent.

## Usage

The `session/prompt` response `_meta` carries `modelId`, `inputTokens`, `outputTokens`, `cachedReadTokens`,
`reasoningTokens` for the turn, and `usage` (session totals with `costUsdTicks` when the cost is known).

## Login

There is no account login. `initialize` advertises no auth methods and `authenticate` is a no-op that succeeds.

## Sessions

- Stored under `<SUPERCHARGE_HOME>/sessions/<percent-encoded cwd>/<session id>/` (`summary.json`, `chat_history.jsonl`, `updates.jsonl`, ...).
- `session/list` takes an optional `cwd` (omit for all projects) and a `cursor`; rows carry `cwd`, `title`, `updatedAt`.
- `_supercharge/sessions/changed` notification: `{ changes: [{ kind: "created"|"updated"|"deleted", sessionId, cwd }] }`,
  emitted within ~2 s when this or any other CLI process changes a session. Writes still go through the CLI.

## Extension names (`_supercharge/*`)

Every legacy `_x.ai/<name>` extension method, notification and capability key has a
`_supercharge/<name>` equivalent. Both are accepted during the migration:

| Direction | Legacy name | Supercharge name | Behaviour now |
|---|---|---|---|
| Client → agent requests and notifications | `_x.ai/<name>` | `_supercharge/<name>` | Both accepted (same handler) |
| Agent → client notifications (WebSocket) | `_x.ai/<name>` | `_supercharge/<name>` | Dual-emitted: the `_supercharge/` frame first, then the legacy one |
| Agent → client requests (e.g. `ask_user_question`, `mcp/elicit`, `mcp/sdk_call`) | `_x.ai/<name>` | — | Legacy name only (a request can't be sent twice) |
| `initialize` capability keys | `x.ai/fs_notify`, `x.ai/hooks`, `x.ai/capabilities`, `x.ai/mcp/sdk`, `x.ai/pluginDirs` | `_supercharge/fs_notify`, `_supercharge/hooks`, `_supercharge/capabilities`, `_supercharge/mcp/sdk`, `_supercharge/pluginDirs` | Both advertised |
| Client `_meta` keys read by the agent | `x.ai/hooks`, `x.ai/fs_notify` | `_supercharge/hooks`, `_supercharge/fs_notify` | Both read (Supercharge name wins) |

Examples: `_x.ai/session_notification` → `_supercharge/session_notification`,
`_x.ai/mcp/server_status` → `_supercharge/mcp/server_status`,
`_x.ai/mcp/servers_updated` → `_supercharge/mcp/servers_updated`,
`_x.ai/interject` → `_supercharge/interject` (also `_supercharge/session/steer`).

Supercharge-only names with no legacy form: `_supercharge/sessions/changed`, `_supercharge/models/list`,
`_supercharge/session/steer`.

Migrate Studio to the `_supercharge/` names and ignore the duplicate legacy notification.
The legacy names will be removed in a later release.

## Reasoning effort

- Config option id `reasoning_effort` (category `thought_level`, a `select`) is returned in `configOptions`
  by `session/new` and `session/load`, for models that support reasoning.
- Values: `off`, `low`, `medium`, `high`, `xhigh`, filtered per model to the levels the provider accepts
  where known (for example some models list only `off`/`low`/`medium`/`high`). `none` is accepted as an alias of `off`.
- Set it with `session/set_config_option` `{ sessionId, configId: "reasoning_effort", value: "high" }`.
  The response carries the updated `configOptions`, and a `session/update` with
  `sessionUpdate: "config_option_update"` (`configOptions`) is also sent. The setting applies to the next model calls.
- On the wire: Chat Completions `reasoning_effort` (`off` → `"none"`), Responses `reasoning.effort`
  (`off` → `"none"`), Messages `thinking: { type: "enabled", budget_tokens }` with
  low 2048 / medium 8192 / high 16384 / xhigh 32768 (kept below `max_tokens`; `off` sends no `thinking`).

## Context usage

After each model call the agent sends the notification `_supercharge/session/context_usage`:

```json
{ "sessionId": "…", "usedTokens": 51234, "maxTokens": 200000 }
```

`usedTokens` = input + cache reads + output tokens of that call (input includes cache writes);
`maxTokens` = the model's context window. Not sent when the provider reports no usage.

## Response completed

`response_completed` is a `sessionUpdate` inside `_x.ai/session_notification`, so it is dual-emitted as
`_supercharge/session_notification` with `update: { sessionUpdate: "response_completed", message_id?, stop_reason?,
usage?: { input_tokens, output_tokens, cache_read_input_tokens, cache_creation_input_tokens, reasoning_tokens }, signature?, stop_sequence? }`.

## Context window

Models advertise their real context window in `availableModels[]._meta.totalContextTokens`
and in `_supercharge/session/context_usage.maxTokens`. For custom providers the window is
read from the provider's `/models` metadata (`context_length`, `context_window`,
`max_input_tokens`, `top_provider.context_length`, ...); 256k is used only when the provider
reports nothing.

Per-model override (applies to `totalContextTokens`, `maxTokens` and the auto-compaction threshold):

- Config: `~/.supercharge/config.toml`

  ```toml
  [model."<model-id>"]
  context_window = 1000000
  ```

- TUI: `/context` shows usage; `/context 1m`, `/context 256k`, `/context 400000` set the
  override for the current model; `/context reset` (or `auto`) clears it. Saved to config.
- ACP session config option `context_window` (select): values `auto`, `128000`, `200000`,
  `256000`, `400000`, `1000000`, `2000000` (plus the current custom value). Set with
  `session/set_config_option` `{ "configId": "context_window", "value": "1000000" }`.
- ACP extension `_supercharge/session/context_window`
  `{ "sessionId": "...", "contextWindow": 1000000 | "1m" | "auto" | null }` →
  `{ "contextWindow": <effective>, "overridden": <bool> }`.

Changes emit `config_option_update` with the refreshed `context_window` option.
