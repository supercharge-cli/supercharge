# Architecture

## Design intent

The implementation is an independent, service-oriented alternative rather than a fork of the upstream command-line interface. It preserves the useful separation of concerns observed in the public grok-build source while keeping the first release small enough to run as a continuously hosted service.

| Upstream boundary | This project | Responsibility |
| --- | --- | --- |
| Agent runtime and prompt/context assembly | `agent/runtime.py` and `agent/model_client.py` | Own a run, optionally execute a tool, then synthesize a response |
| Tool implementations | `agent/execution.py` | Apply a command policy and route execution to local dry-run or E2B |
| Workspace and execution | `E2BBackend` | Create or connect to an isolated sandbox, run a bounded command, collect output, and kill the sandbox |
| Configuration | `agent/config.py` | Load typed environment configuration and hard limits |
| Session/event state | `agent/storage.py` | Persist runs and append-only events in SQLite |
| Headless transport | `agent/main.py` | Expose health, run, status, and event endpoints |

The upstream assessment used commit `c2dab005c2b53c99209dc6faf82153d782f76ee4` from [xai-org/grok-build](https://github.com/xai-org/grok-build). The upstream repository describes its code as a Rust CLI/TUI and agent runtime and distributes first-party code under Apache 2.0; the attribution record is in [`NOTICE-UPSTREAM.md`](../NOTICE-UPSTREAM.md).

## E2B lifecycle boundary

E2B is treated as an execution backend, not as the 24/7 host for the agent service. The service creates a sandbox for a command, runs it with a bounded timeout, captures capped stdout/stderr, and shuts the sandbox down. A caller may provide a sandbox ID to reconnect to an existing workspace, but the service does not assume that any one sandbox stays alive forever.

The [E2B sandbox documentation](https://e2b.dev/docs/sandbox) states that sandboxes have bounded continuous lifetimes and supports pause/resume for longer-lived state. The [Python SDK reference](https://e2b.dev/docs/sdk-reference/python-sdk/v1.0.4/sandbox_async) documents `AsyncSandbox.create`, `connect`, `set_timeout`, `commands.run`, and `kill`. The long-running part of this project is therefore the HTTP/worker process, backed by durable run metadata; E2B sandboxes are disposable, isolated execution environments.

## 24/7 operation

The repository includes two operational shapes:

1. `docker-compose.yml` runs the container with an always-restart policy and a persistent `runtime/` volume.
2. `deploy/hermes-agent.service` runs the application directly under systemd with restart-on-failure, a restricted service user, and a separate environment file.

The production deployment must provide `HERMES_API_TOKEN`, `MODEL_API_KEY`, and `E2B_API_KEY` through a secret store or protected environment file. No production credential belongs in Git. The public health endpoint intentionally returns only liveness information; run endpoints require bearer authentication whenever `HERMES_API_TOKEN` is set.

## Current limitations

The first implementation has a single-process in-memory task map. Run state and events are durable, and active runs are marked failed after a process restart, but the service does not yet requeue incomplete work automatically. Multi-worker deployment should wait until the queue is moved to a shared broker or a database-backed lease protocol.
