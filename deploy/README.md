# 24/7 Rust deployment

The persistent process is the Rust daemon binary. Starting this process is daemon mode: it keeps the HTTP service alive, starts configured gateway loops, persists gateway offsets, job state, memory, and Matrix sync cursors, and is restarted by Docker or systemd after crashes and host reboots. It exposes `/healthz`, `/v1/capabilities`, authenticated `/v1/approval/check`, `/v1/exec`, `/v1/jobs`, `/v1/memory`, `/v1/memory/stats`, `/v1/memory/compact`, `/v1/daemon/status`, `/v1/observability`, `/v1/connections`, `/v1/computer/status`, and `/v1/computer/browser`.
 The execution endpoint requires a build with the `e2b` feature and a valid `E2B_API_KEY` at runtime. With that feature enabled, the daemon also exposes authenticated durable E2B session routes: `POST/GET /v1/e2b/sessions`, `GET/DELETE /v1/e2b/sessions/:id`, `POST /v1/e2b/sessions/:id/exec`, `POST /v1/e2b/sessions/:id/wake`, and `POST /v1/e2b/sessions/:id/pause`. Session metadata is persisted at `SUPERAGENT_E2B_SESSION_STATE`, and `SUPERAGENT_E2B_WORKSPACE_ROOT` defines the default absolute workspace boundary.

## Docker Compose

Create `.env.runtime` locally or through the deployment secret manager. It must not be committed:

```dotenv
SUPERAGENT_API_TOKEN=replace-with-a-long-random-token
E2B_API_KEY=replace-with-e2b-key
SUPERAGENT_JOB_STATE=/var/lib/superagent/jobs.json
SUPERAGENT_MEMORY_STATE=/var/lib/superagent/memory.json
SUPERAGENT_GATEWAY_STATE=/var/lib/superagent/telegram-gateway.json
MATRIX_SYNC_STATE=/var/lib/superagent/matrix-sync.json
# Enable only the gateways you have configured.
TELEGRAM_BOT_TOKEN=
TELEGRAM_ALLOWED_USERS=
TELEGRAM_ALLOWED_CHATS=
TELEGRAM_WEBHOOK_SECRET=
DISCORD_BOT_TOKEN=
DISCORD_ALLOWED_USERS=
DISCORD_ALLOWED_CHANNELS=
SLACK_BOT_TOKEN=
SLACK_APP_TOKEN=
SLACK_ALLOWED_USERS=
SLACK_ALLOWED_CHANNELS=
WHATSAPP_ACCESS_TOKEN=
WHATSAPP_PHONE_NUMBER_ID=
WHATSAPP_VERIFY_TOKEN=
WHATSAPP_APP_SECRET=
WHATSAPP_ALLOWED_USERS=
SIGNAL_ACCOUNT=
SIGNAL_CLI_BASE_URL=http://127.0.0.1:8080
SIGNAL_ALLOWED_SENDERS=
MATRIX_HOMESERVER=
MATRIX_ACCESS_TOKEN=
MATRIX_USER_ID=
MATRIX_ALLOWED_USERS=
MATRIX_ALLOWED_ROOMS=
GENERIC_WEBHOOK_SECRET=
XAI_API_KEY=replace-with-xai-api-key
GROK_DEFAULT_MODEL=replace-with-model-id
GROK_CONFIG_PATH=/var/lib/superagent/grok-config.toml
GROK_AGENT_BINARY=/usr/local/bin/xai-grok-pager-bin
```

Start the service with:

```bash
docker compose up -d --build
docker compose ps
curl http://127.0.0.1:8787/healthz
curl -H "Authorization: Bearer $SUPERAGENT_API_TOKEN" http://127.0.0.1:8787/v1/connections
```

The Compose definition uses an automatic restart policy, a non-root container, a read-only root filesystem, a temporary filesystem for `/tmp`, dropped Linux capabilities, and a local-only port binding. Put a reverse proxy or private network in front of it if remote access is required. Persist `/var/lib/superagent` so job state, memory, offsets, and cursors survive container replacement.

## systemd

Build the binary with E2B support and copy it to `/opt/superagent/bin/xai-grok-superagent-server`. Create `/etc/superagent/superagent.env` with mode `0600`, create the `superagent` user and state directories, install `superagent.service` into `/etc/systemd/system/`, then run:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now superagent.service
sudo systemctl status superagent.service
curl http://127.0.0.1:8787/healthz
curl -H "Authorization: Bearer $SUPERAGENT_API_TOKEN" http://127.0.0.1:8787/v1/connections
```

The systemd unit restarts on failure, handles SIGTERM, protects the host filesystem, and grants write access only to explicitly designated state and log directories. Rotate the API token and E2B key through the host secret-management procedure rather than editing Git-tracked files.

## Model API connection

The gateway adapters deliver authorized messages to the headless agent bridge when `GROK_AGENT_BINARY` is set. The upstream runtime uses the compatibility variable `XAI_API_KEY` for provider authentication and can use `GROK_DEFAULT_MODEL` or the configuration overlay at `GROK_CONFIG_PATH` for model selection. These values belong in the deployment secret/configuration store, not in Git. Without a provider key and valid headless-runtime configuration, gateway transport can still be tested, but model replies will not be available.

## Durable memory

Memory entries are persisted atomically at `SUPERAGENT_MEMORY_STATE` and are scoped by `session_key`. `POST /v1/memory` accepts optional `kind`, `importance`, and `tags` metadata; `GET /v1/memory` performs session-scoped text/tag/kind recall with bounded, importance-aware ranking; `DELETE /v1/memory/:id` removes one entry; `GET /v1/memory/stats` reports entry and session counts; `GET /v1/memory/export` creates a JSON backup; `POST /v1/memory/restore` imports a bounded backup with optional replacement; and `POST /v1/memory/compact` removes the least important, oldest entries above a configured capacity. Secrets and provider credentials are never written into memory automatically by the daemon.

## Session controls

All gateways share persisted control commands through `SUPERAGENT_CONTROL_STATE`. `/new` and `/start` resume a session, `/stop` blocks further agent turns for that session, `/status` reports its control state, `/model [id]` records a requested model for operator inspection, and `/help` lists the commands. These controls are applied before the headless provider bridge and therefore work consistently across the messaging adapters.

## Skills and MCP integrations

Set `SUPERAGENT_SKILLS_ROOT` to a read-only or operator-managed directory containing skill subdirectories with `SKILL.md` manifests. `GET /v1/integrations/skills` discovers manifests, while `GET /v1/integrations/skills/:name` loads one validated manifest, returns its digest, and rejects traversal or oversized files. Set `SUPERAGENT_MCP_SERVERS` to a JSON array of descriptors; `GET /v1/integrations/mcp` validates and reports supported `stdio`, `http`, and `sse` transports, while `GET /v1/integrations/mcp/tools?name=server` performs bounded HTTP/SSE JSON-RPC initialization and `tools/list` discovery for enabled network servers. Authenticated `POST /v1/integrations/mcp/call` invokes a named tool on an enabled HTTP/SSE server with a 256 KiB argument limit, a 1–120 second timeout bound, session-header propagation, and JSON-RPC error handling. The daemon does not execute arbitrary skill code or stdio MCP commands through these HTTP routes.

## Subagents

When the headless agent bridge is configured, `POST /v1/subagents` can run a bounded child turn with a parent session, prompt, depth, and timeout. `SUPERAGENT_MAX_SUBAGENT_DEPTH` and `SUPERAGENT_MAX_PARALLEL_SUBAGENTS` limit recursion and concurrency. Each child receives an isolated session identifier, and output is capped before it is returned. The endpoint is authenticated and returns service-unavailable when no provider bridge is configured.

## Durable schedules

`SUPERAGENT_SCHEDULE_STATE` persists interval schedules across restarts. Authenticated `POST /v1/schedules` creates a schedule with a name, prompt, interval, and optional `home_channel`; `GET /v1/schedules` lists schedules; `POST /v1/schedules/:id/enabled` toggles one; and `DELETE /v1/schedules/:id` removes it. Due schedules are claimed exactly once and enqueued into the durable job store. Set `home_channel` to `telegram:CHAT_ID`, `discord:CHANNEL_ID`, `slack:CHANNEL_ID[:THREAD_TS]`, `whatsapp:PHONE`, `signal:RECIPIENT`, `matrix:ROOM_ID`, `email:ADDRESS`, or `webhook:NAME`; successful results are routed through the configured outbound client instead of being broadcast implicitly. Webhook delivery additionally requires `GENERIC_WEBHOOK_URL`.

## Media handling

Agent replies may include `MEDIA:relative/path|mime/type` markers. Authenticated `POST /v1/media/parse` strips those markers from text and returns validated attachment metadata. `SUPERAGENT_MEDIA_ROOT` defines the only permitted root and `SUPERAGENT_MAX_MEDIA_BYTES` caps file size; absolute paths, traversal, missing files, and files outside the root are rejected. Native Telegram, Discord, and Slack uploads remain the next delivery-layer integration on top of this secure parser.

## Gateway configuration

The daemon starts each adapter when its required environment variables are present. Telegram uses long polling; Discord uses the Gateway WebSocket and REST API; Slack uses Socket Mode and the Web API; WhatsApp uses its verified webhook route at `/webhooks/whatsapp`; Signal uses a signal-cli-compatible REST service; Matrix uses Client-Server `/sync`; email polls IMAP and replies over SMTP; and the generic adapter uses `/webhooks/generic` with `X-Superagent-Webhook-Secret`. The `/v1/daemon/status` endpoint reports enabled gateway names and durable job counts. The authenticated `/v1/observability` endpoint adds uptime, gateway, job, memory, and schedule counters without returning prompts, credentials, or message contents. The authenticated `/v1/connections` endpoint reports whether each configured integration has its required environment variables and lists only missing variable names; it never returns secret values and does not claim that a remote service is reachable until a live API check is performed.

## API protection and quotas

Protected API routes reject requests without `SUPERAGENT_API_TOKEN` when a token is configured, cap request bodies at 25 MiB, emit conservative no-sniff and no-store response headers, and optionally enforce `SUPERAGENT_RATE_LIMIT_PER_MINUTE`. A positive quota applies independently to each `X-Superagent-Session` value, with `api` as the fallback key; the default value `0` disables the limiter. Put the API behind TLS and a network access policy, and persist the runtime directory so durable jobs, schedules, controls, and memory survive daemon restarts.

## Runtime backup and restore

The `deploy/backup-runtime.sh` script archives the known durable JSON state files from `SUPERAGENT_RUNTIME_DIR` (default `/var/lib/superagent`) with restrictive permissions. The matching `deploy/restore-runtime.sh` script validates archive member names before replacing only the supported state files. Stop the daemon before restoring, keep backup archives outside the runtime directory, and encrypt them with the host’s approved secret-management or storage system when they leave the machine.

## Upgrade and rollback

Use `deploy/upgrade.sh /path/to/xai-grok-superagent-server` to stage an executable release under `SUPERAGENT_INSTALL_ROOT` (default `/opt/superagent`), create a pre-upgrade runtime backup, replace the daemon binary atomically, restart an active `superagent.service`, and require a successful `/healthz` check before completing. If the health check fails, the previous binary is restored automatically. `deploy/rollback.sh` restores the previous binary from the recorded release directory; both scripts support `SUPERAGENT_BINARY_PATH`, `SUPERAGENT_SERVICE_NAME`, and `SUPERAGENT_HEALTH_URL` overrides for non-systemd deployments. Keep release directories and runtime backups on the same protected host, and review service logs after every upgrade.

## Telegram gateway

The default Telegram mode is long polling. To use webhook mode instead, expose `/webhooks/telegram` through a public HTTPS reverse proxy, set `TELEGRAM_WEBHOOK_SECRET`, configure Telegram’s `setWebhook` method with the same secret, and forward the `X-Telegram-Bot-Api-Secret-Token` header unchanged. The webhook route rejects requests without the configured secret, applies the same user/chat allowlists, and returns `202 Accepted` after transport processing. Do not run Telegram long polling and webhook mode for the same bot at the same time; Telegram treats them as mutually exclusive update mechanisms.

Copy `.env.runtime.example` to the host secret file and set `TELEGRAM_BOT_TOKEN` from BotFather plus at least one numeric `TELEGRAM_ALLOWED_USERS` or `TELEGRAM_ALLOWED_CHATS` value. When `TELEGRAM_BOT_TOKEN` is present, daemon mode starts the Telegram long-polling loop automatically. The gateway persists its update offset at `SUPERAGENT_GATEWAY_STATE`, retries after transport failures, and denies unallowlisted users and chats by default. Set `TELEGRAM_ECHO_MODE=true` only for transport verification. When `GROK_AGENT_BINARY` is configured, authorized Telegram messages are routed into the headless runner.

## Email gateway

Set `EMAIL_USERNAME`, `EMAIL_PASSWORD`, and a non-empty comma-separated `EMAIL_ALLOWED_SENDERS` list to enable the email loop. Configure `EMAIL_IMAP_HOST`, `EMAIL_IMAP_PORT`, and `EMAIL_IMAP_TLS` for the inbox, plus `EMAIL_SMTP_HOST`, `EMAIL_SMTP_PORT`, `EMAIL_SMTP_TLS`, and optional `EMAIL_FROM` for replies. TLS is enabled by default and uses the host’s native certificate store. The daemon polls unread messages, accepts only allowlisted senders, derives a durable session key from the sender and subject, and marks accepted messages as seen after retrieval. Replies are sent as plain-text `Re:` messages through authenticated SMTP. Use an application password or provider-issued mailbox credential; do not place mailbox credentials in Git or memory backups.

## Providers and custom OpenAI-compatible endpoints

The daemon supports built-in provider profiles through `SUPERAGENT_PROVIDER`, `SUPERAGENT_MODEL`, `SUPERAGENT_PROVIDER_BASE_URL`, `SUPERAGENT_PROVIDER_FORMAT`, and `SUPERAGENT_PROVIDER_API_KEY_ENV`. The registry includes Pollinations, OpenAI, OpenRouter, DeepSeek, Google AI Studio, Groq, Z.AI, Kimi Coding, Qwen Cloud, Routing.run, ZenLLM, and Nous Research. The normal deployment pattern places only the API key in the host or E2B secret store under the configured environment-variable name.

Authenticated `GET /v1/providers` returns the built-in profiles and active selection. Authenticated `POST /v1/providers/config` accepts `provider`, `base_url`, `model`, `format`, and `api_key_env`; it validates HTTPS endpoints, permits plain HTTP only for loopback development services, and writes a nested upstream-compatible model TOML file without accepting a raw API key. Use `provider=custom` for arbitrary OpenAI-compatible endpoints.

## Interactive browser workspace

The Rust daemon’s browser adapter supervises the template’s `agent-browser stream` service on localhost port `9223`. `GET /v1/computer/status` reports stream readiness, while authenticated `GET /v1/computer/browser` emits the same SSE framing consumed by the existing app Computer pane: `status` events, base64 JPEG `frame` events, and keepalive comments. The adapter acknowledges frames for backpressure but deliberately ignores browser input events, making the observer stream view-only. Browser interaction remains an agent-side operation through the headless tool bridge; user input controls should be added only through a separately authorized action endpoint.

## E2B sessions
 lifecycle

The persistent session API follows the reference E2B gateway’s safer lifecycle model: a session creates or reconnects a sandbox, requests provider-side `onTimeout: pause` with `autoResume: true`, refreshes its timeout, keeps the sandbox alive across commands, reconnects paused sandboxes on wake or execution, caps stdout/stderr, and rejects absolute working directories outside the configured workspace or paths containing traversal segments. E2B auto-resume is activity-driven: the next SDK/API operation wakes the paused sandbox; it is not a background promise that runs without any request. The one-shot `/v1/exec` endpoint remains available and still creates and tears down an isolated sandbox; use `/v1/e2b/sessions/:id/exec` when a durable sandbox filesystem and resumable session are required. Pausing is explicit, while deletion kills the remote sandbox and removes the local session record.

## Operational boundary

The Rust HTTP service is designed to run continuously on a persistent Linux host or container platform. E2B sandboxes are created for bounded isolated command execution and are cleaned up after each run. They are not used as the permanent host for the agent control plane.

Before exposing `/v1/exec`, configure an authentication token and retain the approval policy. The service rejects destructive executables and requires explicit approval for network-facing commands.
