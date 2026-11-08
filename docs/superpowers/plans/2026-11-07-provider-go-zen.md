# OpenCode Go and OpenCode Zen providers

Approved scope: two visible `/setup` choices only. Do not import a wider catalog. Do not impersonate another application. Do not use shared unauthenticated keys. Supercharge identity on documented external-client inference only.

## Design

- Provider ids: `opencode-go` and `opencode-zen` (distinct from existing `zenllm`).
- Labels: OpenCode Go, OpenCode Zen.
- Each persists its own key in `[provider_credentials.<id>]` through existing `/setup` secure config. Isolated slot wins; never borrow the other slot, the active global setup key of a different provider, or `OPENCODE_API_KEY`.
- Reuse Chat Completions (`SetupWireFormat::Openai`). Docs say text models work through translated APIs. Do not send `google/v1beta` or systemone.
- Endpoints (docs + reference):
  - Go chat base: `https://opencode.ai/inference/go/openai/v1`
  - Go models: `GET https://opencode.ai/inference/go/v1/models`
  - Zen chat base: `https://opencode.ai/inference/openai/v1`
  - Zen models: `GET https://opencode.ai/inference/v1/models`
- Persist `endpoints.models_list_url` from an explicit override for these ids so setup preview, background fetch, `/models`, and ACP use the list URL, not `{openai/v1}/models`.
- Default/curated ids from published docs only: Go `glm-5.3-flash`; Zen paid `glm-5.3`.
- Zen: drop models whose list metadata shows zero input cost or an explicit free flag; documented official-app free chat models are not advertised. Go keeps documented fallback models even when cost is zero. Unrecognized models without cost metadata are kept without invented prices.
- Exclude non-chat (existing filters plus `jev` / systemone).
- Sampler helper `opencode_inference`: parsed URL must be https, host `opencode.ai`, port 443, path `/inference` or `/inference/...`. Then send `User-Agent: supercharge/<version>` and `x-opencode-session` (stable per `SamplingClient` / conversation; generated once for one-off clients). No copied client/project/UA. Unrelated and lookalike hosts unchanged.
- Catalog GET uses the same header rules on the resolved list URL.

## Files

- `crates/codegen/supercharge-sampler/src/opencode_inference.rs` (new) + `lib.rs` + `client.rs`
- `crates/codegen/supercharge-shell/src/agent/provider_setup.rs`
- `crates/codegen/supercharge-pager/src/views/setup_modal.rs` (row snapshot test)
- this plan

## Tests first (targeted, `--locked`, no live network)

```
source /workspace/env.sh
export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216
cargo test --locked -p supercharge-sampler --lib opencode_inference client::tests -- --nocapture
cargo test --locked -p supercharge-shell --features test-support --lib provider_setup -- --nocapture
cargo test --locked -p supercharge-pager --lib views::setup_modal -- --nocapture
```

Logs: `/workspace/test-logs/go-zen-*.log`

Cover: both setup rows/defaults; isolated key save/load and wrong-key isolation; discovery URL + auth; Zen eligibility vs Go fallbacks; UA and stable/different session headers on canonical URLs; absence on lookalikes; no keys in errors.

## Findings: how the official client gets free models (verified 2026-11-07)

Source: sst/opencode (cloned at v2.2.6), `packages/core/src/plugin/provider/opencode.ts` and `packages/core/src/session/model-request.ts`.

1. Activation: when no `OPENCODE_API_KEY` and no Console connection exist, the plugin sets
   `provider.activation = "enabled"` and `provider.settings.apiKey = "public"` (literal string),
   then disables every model whose catalog cost has `input > 0`. Only zero-input-cost models
   (the `-free` set) remain enabled. With a Console connection or key, the account credential is used.
2. Request identity: every LLM request carries `User-Agent: opencode/<channel>/<version>/<client>`,
   `x-opencode-client`, `x-opencode-project`, `x-opencode-session`, `x-opencode-session-id`,
   `x-session-affinity`, `x-session-id`, `b3`/`traceparent`. Body is plain Chat Completions with
   `stream: true` and `stream_options.include_usage`.
3. Catalog: `https://models.opencode.ai/experimental/v2.0/api.json` (models.dev mirror), UA-gated
   the same way; the `/inference/v1/models` endpoint lists the free models publicly.
4. Enforcement: the free tier is gated on the official client. Verified experimentally on this
   machine, same minute, same IP, same headers, same body: the opencode CLI (its bundled Bun build)
   gets `200` on `glm-5.3-flash-free` even with no account; curl, standalone Bun 1.3.11 and 1.4.4
   (`fetch` direct and forwarding from `Bun.serve`), and any non-CLI client get
   `429 FreeUsageLimitError`. Through a raw mitm the CLI's own request also fails, so the gate is
   connection-level (TLS/HTTP fingerprint of the official binary), not headers.
5. Docs state the policy outright: "Free-tier chat models require OpenCode; use paid models or Go
   from other clients."

Conclusion: replicating free-model access from supercharge would require impersonating the official
client at the TLS layer, which the approved scope above forbids ("Do not impersonate another
application"). The current behavior — hide `cost.input == 0` Zen models, surface paid Zen and Go —
matches both the docs policy and the scope. No code change made from this investigation.

## Follow-up (2026-11-07): opencode gateway for free models — REJECTED

A local-gateway design was fully verified working end to end (spawn `opencode serve`, Basic auth
with the startup password, `POST /api/session` → `POST /api/session/<id>/prompt` → poll
`GET /api/session/<id>/message`; free-model reply at cost 0; `GET /api/model` lists 10 free
models; on-demand CLI download from
`github.com/anomalyco/opencode/releases/download/v<ver>/opencode-linux-<x64|arm64>.tar.gz`,
121.8 MB for v2.2.6). It was REJECTED by decision:

1. The `/prompt` API runs opencode's own agent loop — its system prompt, its tools, its harness.
   supercharge would only pick the model text while opencode does the work. Requirement: all
   prompting, tool use, and agent behavior stay in supercharge's CLI.
2. opencode's server exposes no raw chat-completions passthrough, so it cannot serve as a pure
   API proxy.

Final decision: no proxy, no gateway. Users who want Zen models use the paid Zen provider
(already shipped) or OpenCode Go. Free-tier models remain OpenCode-client-only by OpenCode's own
policy ("Free-tier chat models require OpenCode; use paid models or Go from other clients").
