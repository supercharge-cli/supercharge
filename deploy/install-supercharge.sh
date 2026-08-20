#!/usr/bin/env bash
set -euo pipefail

PREFIX="${SUPERCHARGE_INSTALL_PREFIX:-${HOME}/.local/bin}"
SOURCE="${SUPERCHARGE_BINARY:-./supercharge}"
CONFIG_HOME="${SUPERCHARGE_HOME:-${HOME}/.supercharge}"

if [[ ! -f "$SOURCE" ]]; then
  echo "supercharge binary not found: $SOURCE" >&2
  echo "Download the release asset or set SUPERCHARGE_BINARY=/path/to/supercharge." >&2
  exit 1
fi

mkdir -p "$PREFIX" "$CONFIG_HOME"
install -m 0755 "$SOURCE" "$PREFIX/supercharge"
ln -sfn supercharge "$PREFIX/sc"

cat >&2 <<EOF
Installed Supercharge AI:
  $PREFIX/supercharge
  $PREFIX/sc

Configuration home:
  $CONFIG_HOME

Add this directory to PATH if needed:
  export PATH="$PREFIX:\$PATH"

Start the interactive coding/chat TUI:
  supercharge
  sc

Run a single prompt:
  supercharge --single "Explain this repository"

The same binary retains daemon/headless modes through its existing subcommands.
EOF
