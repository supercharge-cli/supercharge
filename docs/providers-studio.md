# Providers in Supercharge Studio

Studio's **Settings → Providers → Add new provider** must offer the same providers, sign-in
methods and custom endpoints as the CLI's `/setup`, and behave the same way. The rule is simple:

> **The CLI owns every provider.** Studio lists providers, adds custom ones, signs in, stores
> keys and switches models **only** through `supercharge agent serve` ACP calls. Studio keeps no
> provider table, no OAuth client, no token store and no `config.toml` writer of its own.

Every provider (and every mode of a provider, such as Mistral's browser sign-in versus a pasted
key) comes back from the CLI with its auth methods (`device`, `browser`, `none`, `api_key`),
its base URL, wire format and where its models come from. When the CLI gains a provider or a
sign-in method, Studio shows it with no Studio release.

File references are `path:line` in `crates/codegen/supercharge-shell/src/` unless the path
starts with `crates/`. ACP wire names carry a leading underscore (`_supercharge/setup/...`);
the CLI source spells them without it.

## 1. The ACP calls Studio uses

All methods are ACP extension requests on the `supercharge agent serve` WebSocket
(see `docs/acp-studio.md` for connecting and the server key). `initialize` advertises them in
`_meta["_supercharge/setup/methods"]` (`agent/mvp_agent/acp_agent.rs:626`).
The method list is `METHODS` at `extensions/setup.rs:40`.

| Step | Method | Request | Response |
|---|---|---|---|
| List | `_supercharge/setup/providers_list` | `{}` | `{ activeProviderId, activeModelId, configPath, providers: [ProviderInfo] }` |
| Inspect one | `_supercharge/setup/provider_status` | `{ providerId }` | `ProviderInfo` |
| Add custom | `_supercharge/setup/provider_add_custom` | `{ name, baseUrl, format?, apiKey?, id?, validate? }` | `{ provider: ProviderInfo, models: [Model] }` |
| Paste a key | `_supercharge/setup/provider_set_key` | `{ providerId, apiKey, validate? }` | `{ saved, providerId, active, reloaded, models: [Model], provider: ProviderInfo }` |
| Sign in | `_supercharge/setup/auth_start` | `{ providerId, kind? }` | `{ loginId, providerId, kind, url, userCode?, expiresIn, interval }` |
| | `_supercharge/setup/auth_poll` | `{ loginId }` | `AuthLogin` |
| | `_supercharge/setup/auth_cancel` | `{ loginId }` | `{ cancelled }` |
| | notification `_supercharge/setup/auth_changed` | | `AuthLogin` (terminal status only) |
| List models | `_supercharge/setup/models` | `{ providerId?, baseUrl?, apiKey?, format? }` | `{ models: [Model], providerId, baseUrl, format, defaultModel }` |
| Use | `_supercharge/setup/save` | `{ providerId?, baseUrl?, apiKey?, modelId?, modelName?, format?, sessionId? }` | `{ saved, modelId, reloaded, switchedSessions, models, message }` |
| Forget | `_supercharge/setup/provider_remove` | `{ providerId }` | `{ removed, providerId }` |
| Whole state | `_supercharge/setup/status` | `{}` | see `docs/acp-studio.md` |

Handlers: `providers_list`, `provider_status`, `provider_add_custom`, `provider_set_key`,
`provider_remove` in `extensions/setup_providers.rs:392`;
`models` / `save` in `extensions/setup.rs:312`;
`auth_*` in `extensions/setup.rs:486` and `extensions/setup_auth.rs:242`.

### Shapes

`ProviderInfo` (`extensions/setup_providers.rs:99`):

```json
{
  "id": "groq",
  "name": "Groq",
  "description": "Fast Llama inference",
  "baseUrl": "https://api.groq.com/openai/v1",
  "defaultModel": "llama-3.3-70b-versatile",
  "format": "openai",
  "formatName": "Chat Completions",
  "curated": ["..."],
  "custom": false,
  "auth": "api_key",
  "authMethods": ["api_key"],
  "keyOptional": false,
  "signedIn": null,
  "hasApiKey": true,
  "active": false,
  "ready": true,
  "modelsSource": "live",
  "modelsUrl": "https://api.groq.com/openai/v1/models"
}
```

- `authMethods`: every way the provider accepts credentials, interactive kinds first
  (`"device"`, `"browser"`), then `"none"`, then `"api_key"`
  (`extensions/setup_auth.rs:80`). Build the auth UI from this list only.
  - `"device"` / `"browser"`: show a **Sign in** button → `auth_start`.
  - `"api_key"`: show a key field → `provider_set_key`.
  - `"none"`: the key field is optional (local servers).
- `format`: `openai` (Chat Completions), `anthropic` (Messages) or `responses` (Responses);
  `formatName` is the display label (`agent/provider_setup.rs:70`).
- `signedIn`: `true`/`false` for sign-in providers, `null` otherwise.
- `hasApiKey`: the CLI holds a pasted key for this provider (in its own slot, or as the active provider).
- `ready`: the provider can list models now (`keyOptional || hasApiKey || signedIn`).
- `active`: this is the provider `[models] default` runs on.
- `auth`: legacy single value kept for 1.4.19 clients; ignore it in new code.
- `modelsSource` is always `"live"` for listed providers: models come from `modelsUrl` at
  `setup/models` time. There is no static fallback (`agent/provider_setup.rs:1216`).
  `curated` is only the preferred order for the picker.
- No response ever contains an API key, token, refresh token, PKCE verifier or device code.

`Model`: `{ id, name, supportsThinking, contextWindow }` (`contextWindow` may be `null`).

`AuthLogin`: `{ loginId, providerId, kind, status: "pending"|"done"|"denied"|"expired"|"error", interval, message? }`.

Errors are ACP `invalid_params` (bad input, unknown provider, provider rejected the key, login
denied/expired) or `internal_error` (I/O) (`extensions/setup_providers.rs:41`).

## 2. Call sequences

Every flow ends the same way: `setup/models { providerId }` → user picks → `setup/save
{ providerId, modelId }`. `save` writes the config, hot-reloads the catalog and switches the
open sessions (or `sessionId`) to the model (`extensions/setup.rs:324`).
After that, `_supercharge/models/list` and `session/set_model` see the provider's models.

### A. API-key provider (most providers, the routers, custom providers)

1. `providers_list` → row with `authMethods: ["api_key"]`.
2. `provider_set_key { providerId, apiKey }`. The CLI lists the provider's models with that key
   first (`validate: false` skips it); a rejected key fails with `invalid_params` and nothing is saved.
   The key goes to `[provider_credentials.<id>]`; if the provider is active the active entry is
   rewritten too and the catalog reloads (`extensions/setup_providers.rs:432`).
3. `provider_status { providerId }` → `hasApiKey: true, ready: true`.
4. `setup/models { providerId }` (no `apiKey`: the CLI uses the stored one) → `setup/save { providerId, modelId }`.

Passing `apiKey` straight to `setup/models` / `setup/save` also works (the `/setup` flow), but
then the key is stored only when `save` runs.

### B. Device-code sign-in (`nous`)

1. `providers_list` → `authMethods: ["device"]`, `signedIn`.
2. `auth_start { providerId: "nous" }` → show `userCode`, open `url` (on this or any device).
3. `auth_poll { loginId }` every `interval` s, or wait for `auth_changed`, until `status` is terminal.
4. `provider_status { providerId: "nous" }` → `signedIn: true, ready: true`.
5. `setup/models { providerId: "nous" }` → `setup/save { providerId: "nous", modelId }`.

### C. Browser sign-in (`mistral-auth`)

Same as B with `kind` `browser`: `auth_start { providerId: "mistral-auth" }` returns only `url`
(no `userCode`). Open it; the CLI waits for the console and stores the issued key itself.
On `done` the CLI has already made `mistral-auth` the active provider with its default model;
call `setup/models` / `setup/save` to pick another model.

### D. Key-optional local server (`ollama`, `lmstudio`, custom loopback URLs)

1. `providers_list` → `authMethods: ["none", "api_key"]`, `keyOptional: true`, `ready: true`.
2. Optional `provider_set_key` if the server is protected.
3. `setup/models { providerId }` → `setup/save { providerId, modelId }`.

### E. Custom provider

1. `provider_add_custom { name, baseUrl, format?, apiKey? }`. The CLI normalises the URL, checks
   the key, lists the endpoint's models (skip with `validate: false`) and saves the definition. It
   does **not** activate it. Response: `{ provider: ProviderInfo, models }`; `provider.id` is
   `custom-<slug of name>` unless `id` was passed.
2. Later key changes: `provider_set_key { providerId: "custom-…", apiKey }`.
3. `setup/models { providerId: "custom-…" }` → `setup/save { providerId: "custom-…", modelId }`.
   A custom id resolves to its saved URL, format and key (`extensions/setup.rs:118`).
4. `provider_remove { providerId: "custom-…" }` deletes it.

### F. Removing or signing out

`provider_remove { providerId }`: a custom provider's definition, a built-in provider's stored
key (`[provider_credentials.<id>]`), or for `nous` the device sign-in token file. The active
provider cannot be removed (`invalid_params`); switch first with `setup/save`
(`extensions/setup_providers.rs:347`).

## 3. Built-in providers

Source: `SETUP_PROVIDERS` at `agent/provider_setup.rs:173`. The picker list is
`setup_provider_profiles()` (`agent/provider_setup.rs:123`), which
`providers_list` returns in this order. Every listed provider's models come live from its
`modelsUrl` (`GET`, see `agent/provider_setup.rs:1183`); the list URL is
`<baseUrl>/models` when the base already ends in a versioned path, else `<baseUrl>/v1/models`
(`agent/provider_setup.rs:693`)).

| id | Display name | Auth (`authMethods`) | Base URL | Format | Models |
|---|---|---|---|---|---|
| `pollinations` | Pollinations | api_key | `https://gen.pollinations.ai/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:175`) |
| `openscale` | OpenScale | api_key | `https://api.openscale.so/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:190`) |
| `openai` | OpenAI | api_key | `https://api.openai.com/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:199`) |
| `anthropic` | Anthropic | api_key | `https://api.anthropic.com/v1` | Messages | live `/models` (`agent/provider_setup.rs:208`) |
| `openrouter` | OpenRouter | api_key | `https://openrouter.ai/api/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:221`) |
| `deepseek` | DeepSeek | api_key | `https://api.deepseek.com/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:235`) |
| `google` | Google AI Studio | api_key | `https://generativelanguage.googleapis.com/v1beta/openai` | Chat Completions | live `/models` (`agent/provider_setup.rs:244`) |
| `groq` | Groq | api_key | `https://api.groq.com/openai/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:253`) |
| `zai` | Z.AI / GLM | api_key | `https://api.z.ai/api/paas/v4` | Chat Completions | live `/models` (`agent/provider_setup.rs:262`) |
| `kimi-coding` | Kimi Coding | api_key | `https://api.moonshot.ai/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:271`) |
| `kimi` | Moonshot (Kimi Anthropic) | api_key | `https://api.moonshot.ai/anthropic` | Messages | live `/models` (`agent/provider_setup.rs:280`) |
| `qwen` | Qwen Cloud | api_key | `https://dashscope-intl.aliyuncs.com/compatible-mode/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:289`) |
| `mistral-auth` | Mistral Auth | browser | `https://api.mistral.ai/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:298`) |
| `mistral` | Mistral API Key | api_key | `https://api.mistral.ai/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:307`) |
| `together` | Together AI | api_key | `https://api.together.xyz/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:316`) |
| `fireworks` | Fireworks AI | api_key | `https://api.fireworks.ai/inference/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:328`) |
| `grok` | xAI (Grok) | api_key | `https://api.x.ai/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:337`) |
| `routingrun` | Routing.run | api_key | `https://api.routing.run/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:346`) |
| `zenllm` | ZenLLM | api_key | `https://api.zenllm.org/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:355`) |
| `wiro` | Wiro | api_key | `https://llm.wiro.ai/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:364`) |
| `free-ai` | Free.ai | api_key | `https://api.free.ai/v1` | Chat Completions | live `/models`, filtered to grant-eligible self-hosted chat (`agent/provider_setup.rs:373`) |
| `opencode-go` | OpenCode Go | api_key | `https://opencode.ai/inference/go/openai/v1` | Chat Completions | live, dedicated list URL `https://opencode.ai/inference/go/v1/models` (`agent/provider_setup.rs:382`) |
| `opencode-zen` | OpenCode Zen | api_key | `https://opencode.ai/inference/openai/v1` | Chat Completions | live, dedicated list URL `https://opencode.ai/inference/v1/models`, app-only models hidden (`agent/provider_setup.rs:391`) |
| `agentrouter` | AgentRouter | api_key | `https://agentrouter.org/v1` | Responses | live `/models` (`agent/provider_setup.rs:400`) |
| `nous` | Nous Research | device | `https://inference-api.nousresearch.com/v1` | Chat Completions | live `/models` with the sign-in token (`agent/provider_setup.rs:409`) |
| `ollama` | Ollama (local) | none, api_key | `http://127.0.0.1:11434/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:422`) |
| `lmstudio` | LM Studio | none, api_key | `http://127.0.0.1:1234/v1` | Chat Completions | live `/models` (`agent/provider_setup.rs:440`) |

Notes per provider:

- **Routers.** `openrouter` (OpenRouter), `agentrouter` (AgentRouter) and `routingrun`
  (Routing.run) are API-key providers. `openrouter` requests carry an `X-Title: Supercharge AI`
  header (`agent/provider_setup.rs:1177`).
  `agentrouter` uses the Responses format and the CLI adds the identity headers the router
  requires on every request, including the model list
  (`agent/provider_setup.rs:1148`); Studio must not add them itself.
- **Mistral** has two rows, one per mode: `mistral-auth` (browser sign-in, `authMethods: ["browser"]`)
  and `mistral` (pasted key, `["api_key"]`). Both call `https://api.mistral.ai/v1`. Each keeps its
  own credential slot, so signing in never overwrites a pasted key and vice versa
  (`agent/provider_setup.rs:1603`).
- **Nous Research** (`nous`) is device-code only. Its model list is fetched with the signed-in token
  (`agent/provider_setup.rs:1234`).
- **Wiro** (`wiro`) takes `key` or, for signature projects, `key:secret` in the same key field.
- **Free.ai** (`free-ai`) lists only grant-eligible self-hosted chat models from its `/models`
  (`agent/provider_setup.rs:768`).
- **OpenScale** (`openscale`) is a plain Chat Completions key provider.
- `opencode-go` / `opencode-zen` list models from a dedicated URL, not `<baseUrl>/models`
  (`crates/codegen/supercharge-sampler/src/opencode_inference.rs:47`),
  get per-request identity headers from the CLI, and keep isolated key slots. `opencode-zen`
  hides models that only work in the vendor's own app.
- **Anthropic-format** rows (`anthropic`, `kimi`) send `x-api-key` plus `anthropic-version: 2023-06-01`
  (bearer for OAuth-style tokens) (`agent/provider_setup.rs:1141`).
- `ollama`, `lmstudio`: loopback endpoints, so key-optional (`agent/provider_setup.rs:132`).
- One more profile exists only so old saved configs keep loading. It is hidden from the picker
  and from `providers_list` (`agent/provider_setup.rs:123`), and
  `provider_set_key` / `auth_start` reject it. Studio must offer only what `providers_list` returns.

## 4. Custom providers

TUI path: `/setup` → **Custom / paste a base URL** → base URL → format → API key → model list →
model (or type a custom id) → effort → save
(`crates/codegen/supercharge-pager/src/views/setup_modal.rs:37`). Over ACP the
same validation runs in `provider_add_custom`
(`extensions/setup_providers.rs:220`):

| Field | Required | Rules |
|---|---|---|
| `name` | yes | 1–80 printable characters, trimmed (`agent/provider_setup.rs:1696`) |
| `id` | no | defaults to `custom-<slug of name>`; 1–48 of `a-z 0-9 -`, no leading/trailing `-`; must not be a built-in id, `setup` or `custom` (`agent/provider_setup.rs:1673`) |
| `baseUrl` | yes | trimmed, max 500 chars; `https://` added when no scheme; only `http`/`https`; plain `http` only for loopback hosts (`localhost`, `127.0.0.1`, `::1`); a trailing `/models` is dropped (`agent/provider_setup.rs:503`) |
| `format` | no | `openai` (default, also `chat_completions`), `anthropic` (also `messages`), `responses`; anything else is rejected (`extensions/setup_providers.rs:61`) |
| `apiKey` | no | max 8192 chars; text that looks like an auth error message is rejected (`agent/provider_setup.rs:25`); omitted on an update keeps the saved key |
| `validate` | no | default `true`: the CLI lists the endpoint's models before saving and fails on an error or an empty list |

A loopback base URL makes the custom provider key-optional (`authMethods: ["none", "api_key"]`).
Model metadata parsing (ids, names, context windows, thinking support) is shared with the
built-ins (`agent/provider_setup.rs:1080`); at most 500 models,
embedding/image/audio ids are skipped (`agent/provider_setup.rs:721`).

`setup/save` with a bare `baseUrl` (no `providerId`) still works and is saved as the `custom`
provider, the same as the TUI Custom row.

## 5. Sign-in flows

Studio never runs these itself. They are listed so the UI copy and timing match. All run inside
the CLI behind `auth_start` / `auth_poll` / `auth_cancel`; one sign-in at a time, a new
`auth_start` cancels the previous one (`extensions/setup_auth.rs:242`).

### Nous Research: device code (RFC 8628)

Source: `agent/nous_oauth.rs`.

1. `POST https://portal.nousresearch.com/api/oauth/device/code` (form: `client_id`, `scope=inference:invoke`)
   → `device_code`, `user_code`, `verification_uri(_complete)`, `expires_in` (default 900), `interval` (default 5)
   (`agent/nous_oauth.rs:74`).
   The public client id is `DEFAULT_CLIENT_ID` (`agent/nous_oauth.rs:17`),
   overridable with `SUPERCHARGE_NOUS_OAUTH_CLIENT_ID`. There is no client secret.
2. `auth_start` returns `url` = `verification_uri_complete` (or `verification_uri`) and `userCode`
   (`extensions/setup_auth.rs:386`).
3. The CLI polls `POST https://portal.nousresearch.com/api/oauth/token`
   (`grant_type=urn:ietf:params:oauth:grant-type:device_code`, `client_id`, `device_code`).
   `authorization_pending` → keep polling; `slow_down` → interval +5 s (reported in `auth_poll.interval`);
   `expired_token` / `access_denied` → `expired` / `denied` (`agent/nous_oauth.rs:175`).
4. Tokens are saved to `<SUPERCHARGE_HOME>/nous_oauth.json` (`access_token`, `refresh_token`,
   `token_type`, `scope`, `client_id`, `portal_base_url`, `inference_base_url`, `expires_at`),
   mode 0600 on Unix (`agent/nous_oauth.rs:249`).
5. Refresh: 120 s before expiry (JWT `exp` or `expires_in`) the CLI posts `grant_type=refresh_token`
   to the same token URL; `invalid_grant` / `refresh_token_reused` deletes the file and the user
   must sign in again (`agent/nous_oauth.rs:273`). Chats re-read the file when it
   changes, so a new sign-in applies without a restart.

### Mistral: browser sign-in (PKCE S256)

Source: `agent/mistral_browser_sign_in.rs`. Not an OAuth client: the console issues a normal
API key.

1. The CLI makes a PKCE verifier (48 random bytes, base64url) and S256 challenge
   (`agent/mistral_browser_sign_in.rs:146`).
2. `POST https://console.mistral.ai/api/<sign-in path>` with `{ code_challenge, code_challenge_method: "S256" }`
   → `process_id`, `sign_in_url`, `poll_url`, `expires_at` (`agent/mistral_browser_sign_in.rs:276`).
   Both URLs must stay on `console.mistral.ai` or the sign-in stops.
3. `auth_start` returns `url` = `sign_in_url` (no user code); the user approves in the browser.
4. The CLI polls `GET poll_url` every 3 s: `pending`, `completed` (+ `exchange_token`), `expired`,
   `denied`, `error`; 429 backs off 8 s; 3 network failures in a row stop; at most 15 minutes
   (`agent/mistral_browser_sign_in.rs:364`).
5. `POST .../<process_id>/exchange` with `{ exchange_token, code_verifier }` → `api_key`
   (`agent/mistral_browser_sign_in.rs:420`).
6. The key is saved as provider `mistral-auth` with its default model through the `/setup` writer
   (`agent/mistral_browser_sign_in.rs:888`). There is no refresh: the key is
   long-lived; sign in again to replace it.

No other listed provider has a sign-in flow. Every other provider is API key (or none).

## 6. Where config and credentials live

| What | Path | Notes |
|---|---|---|
| Home | `$SUPERCHARGE_HOME`, else `~/.supercharge` (Linux, macOS), `%USERPROFILE%\.supercharge` (Windows) | `crates/codegen/supercharge-dirs/src/lib.rs:94` |
| Config | `<home>/config.toml` | `util/config/mcp.rs:1723`; written atomically, mode 0600 on Unix (`util/config/persist.rs:153`) |
| Device sign-in tokens | `<home>/nous_oauth.json` | 0600 on Unix |
| ACP server key | `<home>/agent-serve.key` | see `docs/acp-studio.md` |

No OS keychain is used for model providers: keys are plain values in `config.toml` (0600) and
tokens in `nous_oauth.json`. (The keychain is used only by plugin connections.) This is
why Studio must not read or write these files: use the ACP calls above.

`config.toml` keys written for providers (`agent/provider_setup.rs:1356`):

```toml
[endpoints]
api_base_url    = "https://api.groq.com/openai/v1"   # active provider
models_base_url = "https://api.groq.com/openai/v1"
models_list_url = "https://api.groq.com/openai/v1/models"

[models]
default = "llama-3.3-70b-versatile"

[model_providers.setup]            # the active provider
id          = "groq"            # built-in id, custom-… id, or "custom"
base_url    = "https://api.groq.com/openai/v1"
api_backend = "chat_completions"   # chat_completions | messages | responses
api_key     = "…"

[model."llama-3.3-70b-versatile"]
model = "llama-3.3-70b-versatile"
name = "…"
base_url = "https://api.groq.com/openai/v1"
api_backend = "chat_completions"
auth_scheme = "bearer"             # x_api_key for Messages
model_provider = "setup"
api_key = "…"

[provider_credentials.deepseek]    # key stored by provider_set_key (not active)
api_key  = "…"
base_url = "https://api.deepseek.com/v1"

[provider_credentials.mistral-auth] # sign-in key; mistral has its own slot with mode = "api_key"
api_key  = "…"
mode     = "browser"
base_url = "https://api.mistral.ai/v1"

[provider_credentials.custom-my-gateway]  # provider_add_custom
custom   = true
name     = "My Gateway"
base_url = "https://gw.example.com/v1"
format   = "openai"
api_key  = "…"
```

Setup never writes `[models] allowed_models`; see `docs/acp-studio.md` for the role-model rules.

## 7. What Studio must not do

- Keep its own provider list, base URLs, formats or model lists. Use `providers_list` and `setup/models`.
- Run any OAuth, device-code or browser sign-in itself, or embed a client id.
- Read or write `config.toml`, `nous_oauth.json` or any credential file.
- Send identity headers for routers; the CLI adds them.
- Show a key field for a provider whose `authMethods` has no `"api_key"`.
