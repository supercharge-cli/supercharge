#!/usr/bin/env bash
# Build Supercharge E2B binaries with a cached Ubuntu 24.04 builder image
# (same glibc as the sandbox template). Do not compile on Fedora/host glibc.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
template_dir="$repo_root/sandbox/e2b-agent-template"
builder_dir="$repo_root/sandbox/e2b-builder"
bin_dir="$template_dir/bin"
mkdir -p "$bin_dir"

builder_tag="${SUPERCHARGE_E2B_BUILDER_TAG:-supercharge-e2b-builder:24.04}"
daemon_only="${SUPERCHARGE_E2B_DAEMON_ONLY:-0}"

build_in_docker() {
  if ! command -v docker >/dev/null 2>&1; then
    return 1
  fi
  echo "Ensuring cached builder image ${builder_tag}…"
  docker build -t "$builder_tag" "$builder_dir"

  echo "Compiling Supercharge E2B assets in ${builder_tag}…"
  local extra=""
  if [ "$daemon_only" != "1" ]; then
    extra='
      cargo build --locked --release -p supercharge-pager-bin --bin supercharge-pager
      install -Dm755 target/release/supercharge-pager /work/sandbox/e2b-agent-template/bin/supercharge
      echo "--- glibc check ---"
      ldd /work/sandbox/e2b-agent-template/bin/supercharge | head -5
      /work/sandbox/e2b-agent-template/bin/supercharge --version
    '
  fi
  docker run --rm \
    -v "$repo_root:/work" \
    -w /work \
    -e CARGO_HOME=/work/.cargo-docker \
    -e RUSTUP_HOME=/work/.rustup-docker \
    -e PATH="/work/.cargo-docker/bin:/opt/cargo/bin:/usr/bin:/bin" \
    "$builder_tag" \
    bash -lc "
      set -euo pipefail
      if ! command -v cargo >/dev/null 2>&1; then
        export PATH=\"/opt/cargo/bin:\${PATH}\"
      fi
      rustc --version
      cargo build --locked --release \\
        -p supercharge-superagent --features e2b --bin supercharge-superagent-server
      install -Dm755 target/release/supercharge-superagent-server \\
        /work/sandbox/e2b-agent-template/bin/supercharge-superagent-server
      ${extra}
    "
}

build_local_cargo() {
  echo "WARNING: local cargo build may link against host glibc and break inside E2B." >&2
  local cargo_bin="${CARGO:-cargo}"
  if ! command -v "$cargo_bin" >/dev/null 2>&1 && command -v rustup >/dev/null 2>&1; then
    cargo_bin="$(rustup which cargo)"
  fi
  cd "$repo_root"
  "$cargo_bin" build --locked --release \
    -p supercharge-superagent --features e2b --bin supercharge-superagent-server
  install -Dm755 \
    "$repo_root/target/release/supercharge-superagent-server" \
    "$bin_dir/supercharge-superagent-server"
  if [ "$daemon_only" != "1" ]; then
    "$cargo_bin" build --locked --release -p supercharge-pager-bin --bin supercharge-pager
    install -Dm755 \
      "$repo_root/target/release/supercharge-pager" \
      "$bin_dir/supercharge"
  fi
}

if [ "${SUPERCHARGE_E2B_BUILD_LOCAL:-}" = "1" ]; then
  build_local_cargo
elif build_in_docker; then
  :
else
  echo "ERROR: Docker build failed and SUPERCHARGE_E2B_BUILD_LOCAL is not set." >&2
  exit 1
fi

printf 'staged Supercharge E2B assets in %s/bin\n' "$template_dir"
ls -la "$bin_dir"
