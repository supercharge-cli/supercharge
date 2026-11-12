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

## Thinking support

Whether a model gets the `reasoning_effort` option is decided in this order:

1. `[model."<id>"]` config: `thinking = true|false`, or `reasoning_efforts = ["high", "medium", "low"]`.
2. Provider `/models` metadata: `supported_parameters` containing `reasoning` / `reasoning_effort` /
   `include_reasoning` / `thinking`, or flags such as `capabilities.reasoning`, `supports_reasoning`,
   `thinking`.
3. A cached capability probe (`~/.supercharge/cache/thinking-probe.json`, per provider base URL + model).
4. Model-name families (gpt-5+, gpt-6, o-series, claude 3.7/4+, gemini 2.5+, deepseek-r1/reasoner,
   qwq/qwen3, gpt-oss, ids containing `reason`/`think`).

When support is still unknown and the user opens the effort picker or sets an effort, the agent
runs a capability probe (at most 6 requests with 16 output tokens, 10 s budget). It tries, in
order, `reasoning_effort`, `reasoning: {effort}`, `thinking: {type: "enabled", budget_tokens}`,
`enable_thinking`, and `include_reasoning` (Responses backends: `reasoning.effort`), stops at the
first one the provider clearly accepts, then checks `xhigh` and `none`. The accepted shape and
levels are cached and used to shape later chat requests.

ACP extension `_supercharge/session/thinking_probe` `{ "sessionId": "...", "force": false }` →
`{ "modelId", "supported", "known", "shape", "efforts", "cached" }`. A successful probe also
emits `config_option_update` with the refreshed `reasoning_effort` option.

## Provider setup (`_supercharge/setup/*`)

Same logic as the TUI `/setup`; writes `~/.supercharge/config.toml` and hot-reloads the model
catalog. Advertised in `initialize` `_meta`: `"supercharge": { "setup": true, "imageSetup": true,
"deviceLogin": true, "authLogin": true }`, `"_supercharge/setup": true`, and `"_supercharge/setup/methods": [...]`.
Errors use ACP `invalid_params` (bad input, provider rejected, login expired/denied) or
`internal_error` (I/O).

| Method | Request | Response |
|---|---|---|
| `_supercharge/setup/status` | `{}` | `{ configured, providerId, baseUrl, format, hasApiKey, modelId, oauth: { nous: { signedIn }, "mistral-auth": { signedIn } }, deviceLoginPending, authLogin: AuthLogin \| null, configPath, providers: [Provider] }` |
| `_supercharge/setup/models` | `{ providerId?, baseUrl?, apiKey?, format? }` | `{ models: [Model], providerId, baseUrl, format, defaultModel }` |
| `_supercharge/setup/save` | `{ providerId?, baseUrl?, apiKey?, modelId?, modelName?, format?, sessionId? }` | `{ saved: true, modelId, reloaded, switchedSessions: [string], models: [Model], message }` |
| `_supercharge/setup/device_start` | `{}` | `{ userCode, verificationUri, verificationUriComplete, expiresIn, interval }` |
| `_supercharge/setup/device_poll` | `{}` | `{ status: "pending" \| "slow_down" \| "done", interval }` |
| `_supercharge/setup/device_cancel` | `{}` | `{ cancelled }` |
| `_supercharge/setup/image_status` | `{}` | `{ configured, baseUrl, hasApiKey, modelId, providers: [Provider] }` |
| `_supercharge/setup/image_save` | `{ providerId?, baseUrl?, apiKey?, modelId?, format? }` | `{ saved: true, modelId }` |
| `_supercharge/setup/auth_start` | `{ providerId, kind?: "device" \| "browser" }` | `{ loginId, providerId, kind, url, userCode?, expiresIn, interval }` |
| `_supercharge/setup/auth_poll` | `{ loginId }` | `AuthLogin` |
| `_supercharge/setup/auth_cancel` | `{ loginId }` | `{ cancelled }` |

- `Provider`: `{ id, name, description, baseUrl, defaultModel, format: "openai"|"anthropic"|"responses", curated: [string], auth: "api_key"|"oauth_device", authMethods: [AuthMethod], keyOptional: bool, signedIn: bool|null }`.
  `auth` is the legacy single value (unchanged since 1.4.19). `authMethods` lists every way the
  provider accepts credentials, in this order: interactive kinds (`"device"`, `"browser"`),
  then `"none"`, then `"api_key"`.
  - No `"api_key"`: the provider takes no pasted key; hide the key field.
  - `"none"` (and `keyOptional: true`): the provider works with no credential, so show the key
    field as optional. `setup/models` / `setup/save` work with no `apiKey`. Today these are the
    local model servers `ollama` and `lmstudio`: a built-in provider counts only when its
    default endpoint is a loopback address, so a remote provider is never key-optional.
  - Only `"api_key"`: a key is required.
  The legacy `auth` value stays `"api_key"` for key-optional providers (as in 1.4.19); read
  `authMethods` / `keyOptional` instead.
  `signedIn` is `null` for key-only providers.
- `AuthLogin`: `{ loginId, providerId, kind, status: "pending"|"done"|"denied"|"expired"|"error", interval, message? }`.
- `Model`: `{ id, name, supportsThinking, contextWindow }` (`contextWindow` may be `null`).
- Target resolution: `providerId` (built-in) supplies `baseUrl`, `format` and the default model;
  `baseUrl` alone means a custom OpenAI-compatible endpoint (`format` defaults to `openai`).
  An omitted `apiKey` reuses the saved key when the saved provider has the same base URL.
  `save` without `modelId` picks the provider default (if listed) or the first listed model.
- `save` applies immediately: it reloads the catalog and switches `sessionId` (or, when omitted,
  every open session) to the saved model with the new key and base URL. A key saved for one
  provider is never sent to another; switching provider needs that provider's `apiKey`.
- Setup never writes `[models] allowed_models`, so every model the provider lists stays
  switchable. A stale list is ignored on load and removed on the next save: one naming only
  setup-saved or Studio models (`studio-<slug>-<16 hex or number>`), one that does not admit `[models] default`, or one
  that blocks every setup-saved model. A hand-written list that admits the default is enforced,
  and the deny error names the file and key.
- Save also points role models (`[models] web_search`, `session_summary`, `prompt_suggestion`,
  `image_description`, `[ui] fork_secondary_model`) at the new default when they name a
  Studio model or a model an earlier setup saved for another endpoint. On load, a role model
  naming a Studio id resolves to `[models] default` when the default is not a Studio model.
- Studio must not write `[models] allowed_models` or any role model key into the user's
  `config.toml`. To use a Studio model, set `[models] default` (or switch the session with
  `session/set_model`); background features follow the session model on their own.
- Device login (Nous, `auth: "oauth_device"`): call `device_start`, show `userCode` and open
  `verificationUri(Complete)`, then call `device_poll` every `interval` seconds (it returns the
  new interval after `slow_down`) until `done`; an expired or denied code returns an error. Then
  call `setup/models` / `setup/save` with `providerId: "nous"` (no `apiKey`).
- Chats against the Nous inference API always use the token currently in `nous_oauth.json`
  (re-read when the file changes, refreshed before expiry), so a new login applies without
  restarting the agent.
- Provider sign-in (`auth_*`, any provider whose `authMethods` has `"device"` or `"browser"`):
  1. `auth_start { providerId }` starts the provider's own flow inside the CLI. Without `kind`
     the provider's first `authMethods` entry is used; an explicit `kind` must be listed for
     that provider. The result always carries `url` (open it locally or show it on another
     device) and, for `device`, a `userCode` to show. Nothing is opened by the CLI itself.
  2. Poll `auth_poll { loginId }` every `interval` seconds, or wait for the
     `_supercharge/setup/auth_changed` notification (payload: `AuthLogin`), sent once when the
     status becomes `done`, `denied`, `expired` or `error`. `slow_down` is handled inside the
     CLI; the reported `interval` grows instead.
  3. On `done`, `setup/status` shows the provider `signedIn: true`; call `setup/models` /
     `setup/save` with that `providerId` and no `apiKey`. The first `auth_poll` that sees `done`
     hot-reloads the catalog.
  4. `auth_cancel { loginId }` stops the flow (`cancelled: false` for an unknown or finished id).
  One sign-in runs at a time: a new `auth_start` cancels the previous one. A cancelled or
  replaced `loginId` makes `auth_poll` fail with `invalid_params`; a finished one keeps
  returning its final status until the next `auth_start`. Unknown providers, key-only
  providers, unsupported kinds and a missing `loginId` fail with `invalid_params`.
- Credentials never cross ACP: responses and notifications carry no token, refresh token, API
  key, PKCE verifier or device code. A device sign-in saves its token where `device_*` does;
  a browser sign-in saves the issued key in `config.toml` under that provider (and makes it the
  active provider with its default model, as the CLI's own sign-in does), so `setup/save` then
  picks the model.
- `device_*` stay as the Nous-only path for existing clients; new clients should use `auth_*`.
