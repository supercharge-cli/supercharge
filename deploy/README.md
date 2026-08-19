# 24/7 Rust deployment

The persistent process is `xai-grok-superagent-server`. Starting this process is daemon mode: it keeps the HTTP service alive, starts configured gateway loops, persists gateway offsets, job state, memory, and Matrix sync cursors, and is restarted by Docker or systemd after crashes and host reboots. It exposes `/healthz`, `/v1/capabilities`, `/v1/approval/check`, `/v1/exec`, authenticated `/v1/jobs`, `/v1/memory`, and `/v1/daemon/status`. The execution endpoint requires a build with the `e2b` feature and a valid `E2B_API_KEY` at runtime.

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
```

The Compose definition uses an automatic restart policy, a non-root container, a read-only root filesystem, a temporary filesystem for `/tmp`, dropped Linux capabilities, and a local-only port binding. Put a reverse proxy or private network in front of it if remote access is required. Persist `/var/lib/superagent` so job state, memory, offsets, and cursors survive container replacement.

## systemd

Build the binary with E2B support and copy it to `/opt/superagent/bin/xai-grok-superagent-server`. Create `/etc/superagent/superagent.env` with mode `0600`, create the `superagent` user and state directories, install `superagent.service` into `/etc/systemd/system/`, then run:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now superagent.service
sudo systemctl status superagent.service
curl http://127.0.0.1:8787/healthz
```

The systemd unit restarts on failure, handles SIGTERM, protects the host filesystem, and grants write access only to explicitly designated state and log directories. Rotate the API token and E2B key through the host secret-management procedure rather than editing Git-tracked files.

## Model API connection

The gateway adapters deliver authorized messages to the grok headless bridge when `GROK_AGENT_BINARY` is set. The upstream grok-build runtime reads `XAI_API_KEY` for xAI authentication and can use `GROK_DEFAULT_MODEL` or the configuration overlay at `GROK_CONFIG_PATH` for model selection. These values belong in the deployment secret/configuration store, not in Git. Without `XAI_API_KEY` and a valid grok runtime configuration, gateway transport can still be tested, but model replies will not be available.

## Gateway configuration

The daemon starts each adapter when its required environment variables are present. Telegram uses long polling; Discord uses the Gateway WebSocket and REST API; Slack uses Socket Mode and the Web API; WhatsApp uses its verified webhook route at `/webhooks/whatsapp`; Signal uses a signal-cli-compatible REST service; Matrix uses Client-Server `/sync`; and the generic adapter uses `/webhooks/generic` with `X-Superagent-Webhook-Secret`. The `/v1/daemon/status` endpoint reports enabled gateway names and durable job counts.

## Telegram gateway

Copy `.env.runtime.example` to the host secret file and set `TELEGRAM_BOT_TOKEN` from BotFather plus at least one numeric `TELEGRAM_ALLOWED_USERS` or `TELEGRAM_ALLOWED_CHATS` value. When `TELEGRAM_BOT_TOKEN` is present, daemon mode starts the Telegram long-polling loop automatically. The gateway persists its update offset at `SUPERAGENT_GATEWAY_STATE`, retries after transport failures, and denies unallowlisted users and chats by default. Set `TELEGRAM_ECHO_MODE=true` only for transport verification. When `GROK_AGENT_BINARY` is configured, authorized Telegram messages are routed into the grok headless runner.

## Operational boundary

The Rust HTTP service is designed to run continuously on a persistent Linux host or container platform. E2B sandboxes are created for bounded isolated command execution and are cleaned up after each run. They are not used as the permanent host for the agent control plane.

Before exposing `/v1/exec`, configure an authentication token and retain the approval policy. The service rejects destructive executables and requires explicit approval for network-facing commands.
