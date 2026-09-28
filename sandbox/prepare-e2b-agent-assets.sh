#!/usr/bin/env bash
# Build Supercharge E2B binaries with a cached Ubuntu 24.04 builder image
# (same glibc as the sandbox template). Do not compile on Fedora/host glibc.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
template_dir="$repo_root/sandbox/e2b-agent-template"
builder_dir="$repo_root/sandbox/e2b-builder"
bin_dir="$template_dir/bin"
browser_runtime_dir="$repo_root/sandbox/browser-runtime"
browser_runtime_stage_dir="$template_dir/browser-runtime"
mkdir -p "$bin_dir"

builder_tag="${SUPERCHARGE_E2B_BUILDER_TAG:-supercharge-e2b-builder:24.04}"
build_cache_root="${SUPERCHARGE_E2B_BUILD_CACHE_ROOT:-$repo_root}"
daemon_only="${SUPERCHARGE_E2B_DAEMON_ONLY:-0}"
# E2B builds from the template directory alone, so stage the source-controlled
# localhost browser sidecar into that context beside the generated binaries.
# Keep dependencies out of the context: the Dockerfile installs them there.
rm -rf "$browser_runtime_stage_dir"
mkdir -p "$browser_runtime_stage_dir"
cp "$browser_runtime_dir/package.json" "$browser_runtime_dir/server.mjs" "$browser_runtime_dir/lease.mjs" "$browser_runtime_dir/start.sh" "$browser_runtime_stage_dir/"

# Pre-fetch the musl ripgrep the CLI build embeds. cargo's 60s GitHub download
# times out on this host; curl retries here and we pass the binary into cargo.
rg_ver="15.0.0"
rg_triple="x86_64-unknown-linux-musl"
vendor_dir="$repo_root/.cache/e2b-vendor"
rg_bin="$vendor_dir/rg-${rg_ver}-${rg_triple}"
mkdir -p "$vendor_dir"
if [ ! -s "$rg_bin" ]; then
  echo "Downloading ripgrep ${rg_ver} (${rg_triple}) for CLI embed…"
  rg_tgz="$vendor_dir/ripgrep-${rg_ver}-${rg_triple}.tar.gz"
  curl -fL --retry 8 --retry-all-errors --retry-delay 2 --connect-timeout 30 --max-time 300 \
    -o "$rg_tgz" \
    "https://github.com/BurntSushi/ripgrep/releases/download/${rg_ver}/ripgrep-${rg_ver}-${rg_triple}.tar.gz"
  unpack="$vendor_dir/unpack-rg"
  rm -rf "$unpack"
  mkdir -p "$unpack"
  tar -xzf "$rg_tgz" -C "$unpack"
  found="$(find "$unpack" -type f -name rg | head -n 1)"
  if [ -z "$found" ]; then
    echo "ripgrep archive did not contain rg" >&2
    exit 1
  fi
  cp "$found" "$rg_bin"
  chmod +x "$rg_bin"
fi

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
      install -Dm755 "$CARGO_TARGET_DIR/release/supercharge-pager" /work/sandbox/e2b-agent-template/bin/supercharge
      echo "--- glibc check ---"
      ldd /work/sandbox/e2b-agent-template/bin/supercharge | head -5
      /work/sandbox/e2b-agent-template/bin/supercharge --version
    '
  fi
  mkdir -p "$build_cache_root/.cargo-docker" "$build_cache_root/.rustup-docker" "$build_cache_root/target-e2b-release"
  docker run --rm \
    -v "$repo_root:/work" \
    -v "$build_cache_root/.cargo-docker:/work/.cargo-docker" \
    -v "$build_cache_root/.rustup-docker:/work/.rustup-docker" \
    -v "$build_cache_root/target-e2b-release:/work/target-e2b-release" \
    -w /work \
    -e CARGO_HOME=/work/.cargo-docker \
    -e RUSTUP_HOME=/work/.rustup-docker \
    -e CARGO_TARGET_DIR=/work/target-e2b-release \
    -e CARGO_NET_RETRY=8 \
    -e SUPERCHARGE_TOOLS_BUNDLE_RG_PATH="/work/.cache/e2b-vendor/rg-${rg_ver}-${rg_triple}" \
    -e SUPERCHARGE_SHELL_BUNDLE_RG_PATH="/work/.cache/e2b-vendor/rg-${rg_ver}-${rg_triple}" \
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
      install -Dm755 \"\$CARGO_TARGET_DIR/release/supercharge-superagent-server\" \\
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

printf 'staged Supercharge E2B assets in %s\n' "$template_dir"
ls -la "$bin_dir" "$browser_runtime_stage_dir"
