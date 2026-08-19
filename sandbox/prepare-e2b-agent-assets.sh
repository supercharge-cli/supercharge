#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
template_dir="$repo_root/sandbox/e2b-agent-template"

cd "$repo_root"
cargo +1.94.0 build --locked --release \
  -p xai-grok-superagent --features e2b --bin xai-grok-superagent-server
cargo +1.94.0 build --locked --release -p xai-grok-pager-bin

install -Dm755 \
  "$repo_root/target/release/xai-grok-superagent-server" \
  "$template_dir/bin/superagent-server"
install -Dm755 \
  "$repo_root/target/release/xai-grok-pager-bin" \
  "$template_dir/bin/superagent-provider-bridge"

printf 'staged Rust agent assets in %s/bin\n' "$template_dir"
