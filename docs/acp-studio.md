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
