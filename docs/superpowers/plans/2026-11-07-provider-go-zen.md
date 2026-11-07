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
