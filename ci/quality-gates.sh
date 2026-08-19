#!/usr/bin/env bash
set -euo pipefail

rust_toolchain="${RUST_TOOLCHAIN:-1.94.0}"
package="${SUPERAGENT_PACKAGE:-xai-grok-superagent}"
features="${SUPERAGENT_FEATURES:-e2b}"

cargo "+${rust_toolchain}" fmt --all -- --check
cargo "+${rust_toolchain}" clippy --locked -p "$package" --features "$features" -- -D warnings
cargo "+${rust_toolchain}" test --locked -p "$package" --features "$features"

printf 'quality gates passed for %s (%s)\n' "$package" "$features"
