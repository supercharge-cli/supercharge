# 24/7 Rust deployment

The persistent process is the Rust daemon binary. Starting this process is daemon mode: it keeps the HTTP service alive, starts configured gateway loops, persists gateway offsets, job state, memory, and Matrix sync cursors, and is restarted by Docker or systemd after crashes and host reboots. It exposes `/healthz`, `/v1/capabilities`, authenticated `/v1/approval/check`, `/v1/exec`, `/v1/jobs`, `/v1/memory`, `/v1/memory/stats`, `/v1/memory/compact`, `/v1/daemon/status`, and `/v1/connections`. The execution endpoint requires a build with the `e2b` feature and a valid `E2B_API_KEY` at runtime. With that feature enabled, the daemon also exposes authenticated durable E2B session routes: `POST/GET /v1/e2b/sessions`, `GET/DELETE /v1/e2b/sessions/:id`, `POST /v1/e2b/sessions/:id/exec`, `POST /v1/e2b/sessions/:id/wake`, and `POST /v1/e2b/sessions/:id/pause`. Session metadata is persisted at `SUPERAGENT_E2B_SESSION_STATE`, and `SUPERAGENT_E2B_WORKSPACE_ROOT` defines the default absolute workspace boundary.

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

Memory entries are persisted atomically at `SUPERAGENT_MEMORY_STATE` and are scoped by `session_key`. `POST /v1/memory` accepts optional `kind`, `importance`, and `tags` metadata; `GET /v1/memory` performs session-scoped text/tag/kind recall with bounded, importance-aware ranking; `DELETE /v1/memory/:id` removes one entry; `GET /v1/memory/stats` reports entry and session counts; and `POST /v1/memory/compact` removes the least important, oldest entries above a configured capacity. Secrets and provider credentials are never written into memory automatically by the daemon.

## Session controls

All gateways share persisted control commands through `SUPERAGENT_CONTROL_STATE`. `/new` and `/start` resume a session, `/stop` blocks further agent turns for that session, `/status` reports its control state, `/model [id]` records a requested model for operator inspection, and `/help` lists the commands. These controls are applied before the headless provider bridge and therefore work consistently across the messaging adapters.

## Skills and MCP integrations

Set `SUPERAGENT_SKILLS_ROOT` to a read-only or operator-managed directory containing skill subdirectories with `SKILL.md` manifests. `GET /v1/integrations/skills` discovers manifests, while `GET /v1/integrations/skills/:name` loads one validated manifest, returns its digest, and rejects traversal or oversized files. Set `SUPERAGENT_MCP_SERVERS` to a JSON array of descriptors; `GET /v1/integrations/mcp` validates and reports only supported `stdio`, `http`, and `sse` transports. These endpoints provide controlled inspection and configuration; they do not silently execute arbitrary skill code or MCP tools.

## Gateway configuration

The daemon starts each adapter when its required environment variables are present. Telegram uses long polling; Discord uses the Gateway WebSocket and REST API; Slack uses Socket Mode and the Web API; WhatsApp uses its verified webhook route at `/webhooks/whatsapp`; Signal uses a signal-cli-compatible REST service; Matrix uses Client-Server `/sync`; and the generic adapter uses `/webhooks/generic` with `X-Superagent-Webhook-Secret`. The `/v1/daemon/status` endpoint reports enabled gateway names and durable job counts. The authenticated `/v1/connections` endpoint reports whether each configured integration has its required environment variables and lists only missing variable names; it never returns secret values and does not claim that a remote service is reachable until a live API check is performed.

## Telegram gateway

The default Telegram mode is long polling. To use webhook mode instead, expose `/webhooks/telegram` through a public HTTPS reverse proxy, set `TELEGRAM_WEBHOOK_SECRET`, configure Telegram’s `setWebhook` method with the same secret, and forward the `X-Telegram-Bot-Api-Secret-Token` header unchanged. The webhook route rejects requests without the configured secret, applies the same user/chat allowlists, and returns `202 Accepted` after transport processing. Do not run Telegram long polling and webhook mode for the same bot at the same time; Telegram treats them as mutually exclusive update mechanisms.

Copy `.env.runtime.example` to the host secret file and set `TELEGRAM_BOT_TOKEN` from BotFather plus at least one numeric `TELEGRAM_ALLOWED_USERS` or `TELEGRAM_ALLOWED_CHATS` value. When `TELEGRAM_BOT_TOKEN` is present, daemon mode starts the Telegram long-polling loop automatically. The gateway persists its update offset at `SUPERAGENT_GATEWAY_STATE`, retries after transport failures, and denies unallowlisted users and chats by default. Set `TELEGRAM_ECHO_MODE=true` only for transport verification. When `GROK_AGENT_BINARY` is configured, authorized Telegram messages are routed into the headless runner.

## E2B session lifecycle

The persistent session API follows the reference E2B gateway’s safer lifecycle model: a session creates or reconnects a sandbox, requests provider-side `onTimeout: pause` with `autoResume: true`, refreshes its timeout, keeps the sandbox alive across commands, reconnects paused sandboxes on wake or execution, caps stdout/stderr, and rejects absolute working directories outside the configured workspace or paths containing traversal segments. E2B auto-resume is activity-driven: the next SDK/API operation wakes the paused sandbox; it is not a background promise that runs without any request. The one-shot `/v1/exec` endpoint remains available and still creates and tears down an isolated sandbox; use `/v1/e2b/sessions/:id/exec` when a durable sandbox filesystem and resumable session are required. Pausing is explicit, while deletion kills the remote sandbox and removes the local session record.

## Operational boundary

The Rust HTTP service is designed to run continuously on a persistent Linux host or container platform. E2B sandboxes are created for bounded isolated command execution and are cleaned up after each run. They are not used as the permanent host for the agent control plane.

Before exposing `/v1/exec`, configure an authentication token and retain the approval policy. The service rejects destructive executables and requires explicit approval for network-facing commands.
