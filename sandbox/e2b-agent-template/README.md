# Rust Agent Interactive E2B Template

This is the **Supercharge** E2B template for Vectra Cloud. It keeps the proven interactive development image from the existing infrastructure stack and runs the Rust Supercharge agent runtime from this repository.

## Included runtime

The image contains `supercharge-superagent-server`, the `supercharge` / `sc` CLI, E2B session support, durable memory and job state, interval scheduling, messaging adapters, `agent-browser` with Chromium, Node.js 20, Bun, pnpm, Python 3 with `uv`, Git, Git LFS, GitHub CLI, and common shell utilities.

Installed paths match the Vectra contract:

| Path | Purpose |
| --- | --- |
| `/usr/local/bin/supercharge-superagent-server` | App-compatible daemon on port `8790` |
| `/usr/local/bin/supercharge` | Interactive CLI |
| `/usr/local/bin/sc` | Symlink to `supercharge` |
| `/opt/vectra/wake-http/start.sh` | Wake/file-share doorbell on port `8788` |

Provider configuration follows the app contract: choose a built-in profile with `SUPERAGENT_PROVIDER`, `SUPERAGENT_MODEL`, and `SUPERAGENT_PROVIDER_API_KEY_ENV`, or use `SUPERAGENT_PROVIDER=custom` with an HTTPS `SUPERAGENT_PROVIDER_BASE_URL`, `SUPERAGENT_PROVIDER_FORMAT=openai`, and a model ID.

The browser screencast service uses port `9223`. Durable state is stored under `/var/lib/superagent`. The default workspace boundary is `/home/user/workspace`.

## Prepare binaries

From the repository root:

```bash
./sandbox/prepare-e2b-agent-assets.sh
```

This uses the cached Docker image `supercharge-e2b-builder:24.04` (`sandbox/e2b-builder/Dockerfile`): Ubuntu 24.04, `rustc`, `protoc`, and C build deps. The first `docker build` of that image is slow; later compiles skip apt/rustup and reuse `target/` plus `.cargo-docker/`.

Daemon-only (skip the large CLI binary):

```bash
SUPERCHARGE_E2B_DAEMON_ONLY=1 ./sandbox/prepare-e2b-agent-assets.sh
```

Do not put the Rust toolchain in the **runtime** E2B template — every customer sandbox would pay for rustc. Tools live in the builder image; the template only gets the two binaries.

## Build and publish

With Docker and `E2B_API_KEY` configured:

```bash
export E2B_API_KEY=...
./sandbox/publish-e2b-template.sh
```

Or manually from this directory:

```bash
e2b template create supercharge-full-sandbox -d Dockerfile --cpu-count 16 --memory-mb 16384
e2b template publish supercharge-full-sandbox -y
```

After publishing, set the gateway env var for the second runtime option:

```bash
SUPERCHARGE_E2B_TEMPLATE=supercharge-full-sandbox
```

The existing XibeCode template remains available via `XIBECODE_E2B_TEMPLATE`.

## Lifecycle contract

Use `onTimeout=pause` and `autoResume=true`. Gateway wake, execution, file access, and HTTP traffic should be treated as activity that can resume a paused sandbox.

| Endpoint | Purpose |
| --- | --- |
| `GET /healthz` | Rust daemon health |
| `GET /v1/computer/status` | Browser stream readiness |
| `GET /v1/computer/browser` | SSE live JPEG frame stream |
| `GET /health` on port `8788` | Wake/file-share service health |
| `POST /wake` on port `8788` | Soft-wake or fast-restart after idle pause |
| `POST /share` on port `8788` | Mint a capability-scoped public workspace file URL |
