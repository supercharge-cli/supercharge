#!/usr/bin/env bash
# Build Supercharge E2B binaries on Ubuntu 24.04 (same glibc as the template image).
# Fedora/other hosts produce GLIBC_2.43+ binaries that fail inside the sandbox.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
template_dir="$repo_root/sandbox/e2b-agent-template"
bin_dir="$template_dir/bin"
mkdir -p "$bin_dir"

e2b_image="${SUPERCHARGE_E2B_BUILD_IMAGE:-ubuntu:24.04}"

build_in_docker() {
  if ! command -v docker >/dev/null 2>&1; then
    return 1
  fi
  echo "Building Supercharge E2B assets in Docker (${e2b_image})…"
  docker run --rm \
    -v "$repo_root:/work" \
    -w /work \
    -e CARGO_HOME=/work/.cargo-docker \
    -e RUSTUP_HOME=/work/.rustup-docker \
    "$e2b_image" \
    bash -lc '
      set -euo pipefail
      export DEBIAN_FRONTEND=noninteractive
      apt-get update -qq
      apt-get install -y -qq \
        curl ca-certificates git build-essential pkg-config libssl-dev \
        clang cmake perl protobuf-compiler
      export RUSTUP_HOME=/work/.rustup-docker
      export CARGO_HOME=/work/.cargo-docker
      export PATH="/work/.cargo-docker/bin:${PATH}"
      if ! command -v cargo >/dev/null 2>&1; then
        curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y -q --default-toolchain stable
      fi
      rustc --version
      cargo build --locked --release \
        -p supercharge-superagent --features e2b --bin supercharge-superagent-server
      cargo build --locked --release -p supercharge-pager-bin --bin supercharge-pager
      install -Dm755 target/release/supercharge-superagent-server /work/sandbox/e2b-agent-template/bin/supercharge-superagent-server
      install -Dm755 target/release/supercharge-pager /work/sandbox/e2b-agent-template/bin/supercharge
      echo "--- glibc check (must not require GLIBC_2.43+) ---"
      ldd /work/sandbox/e2b-agent-template/bin/supercharge | head -5
      /work/sandbox/e2b-agent-template/bin/supercharge --version
    '
}

build_local_cargo() {
  echo "WARNING: local cargo build may link against host glibc and break inside E2B." >&2
  echo "Prefer Docker (ubuntu:24.04) or set SUPERCHARGE_E2B_BUILD_IMAGE." >&2
  local cargo_bin="${CARGO:-cargo}"
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
}

if [ "${SUPERCHARGE_E2B_BUILD_LOCAL:-}" = "1" ]; then
  build_local_cargo
elif build_in_docker; then
  :
else
  echo "ERROR: Docker build failed and SUPERCHARGE_E2B_BUILD_LOCAL is not set." >&2
  echo "Install Docker or set SUPERCHARGE_E2B_BUILD_LOCAL=1 to build on the host (not recommended)." >&2
  exit 1
fi

printf 'staged Supercharge E2B assets in %s/bin\n' "$template_dir"
ls -la "$bin_dir"
