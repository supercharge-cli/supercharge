# Vendored E2B Rust client

This directory contains source derived from [`chwzr/e2b-rs`](https://github.com/chwzr/e2b-rs) at commit `628717577c3574b7ef84349fb190f79d3129a940`.

The upstream project states that it is MIT-licensed and describes itself as a Rust port of the official E2B JavaScript SDK. This copy is vendored so the project can apply a compatibility fix to the upstream workspace dependency declaration: the original manifest requested the removed `reqwest` feature `webpki-roots`, while the private grok-build workspace uses a current `reqwest` release. The vendored manifest removes only that unsupported feature; the Rust SDK source is otherwise retained for the adapter.

The E2B Rust client is not an official SDK published by E2B in this repository. Live E2B integration tests still require a valid `E2B_API_KEY` and are kept separate from deterministic unit tests.
