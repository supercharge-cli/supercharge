# Hermes Agent Alternative

A private, service-oriented AI agent alternative inspired by the runtime boundaries and tool architecture of [xai-org/grok-build](https://github.com/xai-org/grok-build). The project is being designed for E2B-backed execution, provider-neutral model calls, durable session state, and unattended operation.

## Project direction

This repository is not an attempt to impersonate or bypass any proprietary service. It is an independent implementation that uses the public upstream project as an architectural reference. The initial target is a lightweight agent gateway that can accept a task, run an isolated workspace through E2B, invoke a configured model provider, execute approved tools, persist session events, and recover safely after restarts.

The design keeps the major boundaries visible in the upstream source—agent runtime, tools, workspace/execution, configuration, and transport—while using a smaller service footprint suitable for continuous hosting.

## Planned capabilities

| Area | Initial target |
| --- | --- |
| Agent runtime | Session loop with bounded tool calls, cancellation, retries, and structured events |
| Execution | E2B sandbox adapter with local dry-run mode for tests |
| Tools | Read/write files, run commands, inspect workspace, and explicit approval gates |
| Model providers | OpenAI-compatible HTTP provider with a provider interface for future adapters |
| Persistence | SQLite event log and resumable session metadata |
| Operations | Health endpoint, structured logs, graceful shutdown, restart-safe worker loop |
| Deployment | Container image and a 24/7 process definition for a persistent host |
| Security | Secret-by-environment, workspace isolation, command allow/deny policy, request authentication |

## Development status

The repository is being built in incremental commits. The first commit establishes project ownership, source attribution, and the implementation contract. Subsequent commits will add the runtime, E2B adapter, durable storage, operational packaging, and tests.

## Upstream reference

The upstream source is maintained at [xai-org/grok-build](https://github.com/xai-org/grok-build). Its first-party code is distributed under the Apache License, Version 2.0. See [`NOTICE-UPSTREAM.md`](NOTICE-UPSTREAM.md) for attribution and the current source revision used during assessment.

## Local development

The implementation will target Python 3.11+ for the service layer and will keep the E2B SDK optional during local tests. Copy `.env.example` to `.env`, set a local development token if needed, and run the test suite with `pytest` once the first runtime commit lands.

## 24/7 operation

The default deployment shape is a single long-running worker and HTTP health server, backed by durable storage and an external E2B control plane. The service must be run on a host that remains online; the repository will include container and process-manager configuration, but credentials and production infrastructure remain outside Git.

## License

Project-specific code will be licensed separately in a later commit. Upstream-derived portions, if any, will retain the applicable Apache License and attribution notices. No secrets, access tokens, session cookies, or private workspace contents belong in this repository.
