#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
template_dir="$repo_root/sandbox/e2b-agent-template"
bin_dir="$template_dir/bin"
mkdir -p "$bin_dir"

cargo_bin="${CARGO:-cargo}"
if ! command -v "$cargo_bin" >/dev/null 2>&1 && command -v rustup >/dev/null 2>&1; then
  cargo_bin="$(rustup which cargo)"
fi

cd "$repo_root"
"$cargo_bin" build --locked --release \
  -p supercharge-superagent --features e2b --bin supercharge-superagent-server
"$cargo_bin" build --locked --release -p supercharge-pager-bin --bin supercharge-pager

install -Dm755 \
  "$repo_root/target/release/supercharge-superagent-server" \
  "$bin_dir/supercharge-superagent-server"
install -Dm755 \
  "$repo_root/target/release/supercharge-pager" \
  "$bin_dir/supercharge"

printf 'staged Supercharge E2B assets in %s/bin\n' "$template_dir"
