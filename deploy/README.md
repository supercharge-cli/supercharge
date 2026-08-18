# 24/7 deployment

The service process must run on a machine or managed host that remains online. E2B supplies isolated execution sandboxes; it is not assumed to be the permanent host for this HTTP service.

## Container deployment

Create a production `.env` from the repository template and replace every placeholder with a secret from your deployment secret store. Then run:

```sh
mkdir -p runtime
chmod 700 runtime
docker compose up -d --build
docker compose ps
curl http://127.0.0.1:8080/healthz
```

For an internet-facing deployment, place the service behind TLS and a reverse proxy, restrict inbound traffic to the proxy, and keep `HERMES_API_TOKEN` enabled. Do not expose the raw container port without authentication.

## Direct systemd deployment

On a Linux host, create a dedicated `hermes` user, clone the private repository into `/opt/hermes-agent-alternative`, create a Python virtual environment, and install the project with the E2B extra:

```sh
python3 -m venv /opt/hermes-agent-alternative/.venv
/opt/hermes-agent-alternative/.venv/bin/pip install -e '/opt/hermes-agent-alternative[e2b]'
install -d -o hermes -g hermes /opt/hermes-agent-alternative/runtime
install -d -m 700 /etc/hermes-agent
install -m 600 .env /etc/hermes-agent/hermes-agent.env
install -m 644 deploy/hermes-agent.service /etc/systemd/system/hermes-agent.service
systemctl daemon-reload
systemctl enable --now hermes-agent
systemctl status hermes-agent
```

The unit is deliberately configured for automatic restart and graceful shutdown. Use `journalctl -u hermes-agent -f` for logs and `curl http://127.0.0.1:8080/healthz` for a liveness check.

## Important E2B constraint

Keep commands bounded with `E2B_TIMEOUT_SECONDS` and `MAX_COMMAND_SECONDS`. The adapter creates and kills sandboxes per execution by default, which avoids treating an E2B sandbox as an indefinite process host. If a future feature needs resumable workspaces, implement pause/resume and persist the sandbox ID with an explicit lease and cleanup policy.
