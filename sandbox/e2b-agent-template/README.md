# Rust Agent Interactive E2B Template

This is the second E2B template for the platform. It keeps the proven interactive development image from the existing infrastructure stack, but replaces the previous product daemon with the Rust-first agent runtime from this repository.

## Included runtime

The image contains the Rust daemon, its headless provider bridge, E2B session support, durable memory and job state, interval scheduling, all configured messaging adapters, `agent-browser` with Chromium, Node.js 20, Bun, pnpm, Python 3 with `uv`, Git, Git LFS, GitHub CLI, `curl`, `wget`, `jq`, `rg`, `fd`, `fzf`, `tree`, `tmux`, archive tools, process diagnostics, and common browser libraries.

Provider configuration follows the app contract: choose a built-in profile with `SUPERAGENT_PROVIDER`, `SUPERAGENT_MODEL`, and `SUPERAGENT_PROVIDER_API_KEY_ENV`, or use `SUPERAGENT_PROVIDER=custom` with an HTTPS `SUPERAGENT_PROVIDER_BASE_URL`, `SUPERAGENT_PROVIDER_FORMAT=openai`, and a model ID. `GET /v1/providers` lists built-in profiles and the active profile; authenticated `POST /v1/providers/config` writes an upstream-compatible model TOML file while never accepting or persisting a raw API key. Local HTTP endpoints are permitted only for loopback development services.

The agent runs on port `8790` with authenticated `/v1/*` APIs. The E2B wake and file-share service runs on port `8788`; its `GET /health`, `POST /wake`, and `POST /share` routes are the template doorbell and capability-scoped workspace-share contract, while `GET /f/:token/:name` streams a minted file. When the E2B SDK host helper is unavailable, the public form is `https://{port}-{sandboxId}.e2b.app`; the cloud gateway normally resolves the canonical host through `getHost(port)`. The local browser screencast service uses port `9223`, while the daemon exposes the app-compatible SSE stream at `/v1/computer/browser` and status at `/v1/computer/status`. The existing Vectra dashboard exposes the browser stream, raw preview links, the terminal Command pane, workspace browsing/editing, share links, and workspace archive export. The SSE bridge is view-only: browser input events are never forwarded from the observer stream.

Durable state is stored under `/var/lib/superagent`. The default workspace boundary is `/home/user/workspace`. The runtime is non-root and should receive credentials through E2B environment injection or the hosting control plane, never by baking them into the image.

## Prepare binaries

From the repository root, build the release binaries and copy them into the template context:

```bash
./sandbox/prepare-e2b-agent-assets.sh
```

This produces `bin/superagent-server` and `bin/superagent-provider-bridge`. The binaries are build outputs and should not be committed to Git.

## Build and publish

From this repository, with Docker and `E2B_API_KEY` configured:

```bash
pnpm dlx @e2b/cli@latest template create superagent-interactive-sandbox \
  -p sandbox/e2b-agent-template \
  -d Dockerfile \
  --cpu-count 4 \
  --memory-mb 8192
```

After publishing, set the resulting alias or template ID in the cloud gateway as the second-template value. The existing template remains available for rollback.

## Lifecycle contract

Use `onTimeout=pause` and `autoResume=true`. Gateway wake, execution, file access, and HTTP traffic should be treated as activity that can resume a paused sandbox. A hosting supervisor should stop the daemon cleanly before destructive recreation and preserve `/var/lib/superagent` and `/home/user/workspace` when persistence is required.

The user-facing app can proxy these endpoints from the sandbox through the gateway:

| Endpoint | Purpose |
| --- | --- |
| `GET /healthz` | Rust daemon health |
| `GET /v1/computer/status` | Browser stream readiness |
| `GET /v1/computer/browser` | SSE live JPEG frame stream for the Computer pane |
| `GET /v1/observability` | Durable runtime counters and uptime |
| `GET /v1/providers` | Built-in and active provider profiles |
| `POST /v1/providers/config` | Persist a validated provider profile without storing secrets |
| `POST /v1/e2b/sessions` | Create a durable E2B session when the daemon controls child sandboxes |
| `POST /v1/e2b/sessions/:id/exec` | Execute bounded commands in a durable session |
| `GET /health` on port `8788` | Wake/file-share service health and daemon liveness |
| `POST /wake` on port `8788` | Soft-wake or fast-restart the Rust daemon after sandbox idle pause |
| `POST /share` on port `8788` | Mint a capability-scoped public workspace file URL |
| `GET /f/:token/:name` on port `8788` | Stream a previously shared workspace file |

The public E2B preview URL should be issued by the gateway, not embedded into agent responses. Viewer authorization belongs at the cloud gateway and app session layer.
