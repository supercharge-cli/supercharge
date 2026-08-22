#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
template_dir="$repo_root/sandbox/e2b-agent-template"

cd "$repo_root"
cargo +1.94.0 build --locked --release \
  -p supercharge-superagent --features e2b --bin supercharge-superagent-server
cargo +1.94.0 build --locked --release -p supercharge-pager-bin

install -Dm755 \
  "$repo_root/target/release/supercharge-superagent-server" \
  "$template_dir/bin/superagent-server"
install -Dm755 \
  "$repo_root/target/release/supercharge-pager-bin" \
  "$template_dir/bin/superagent-provider-bridge"

printf 'staged Rust agent assets in %s/bin\n' "$template_dir"
