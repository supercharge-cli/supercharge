#!/usr/bin/env bash
set -euo pipefail

install_root="${SUPERAGENT_INSTALL_ROOT:-/opt/superagent}"
binary_path="${SUPERAGENT_BINARY_PATH:-$install_root/bin/superagent-server}"
service_name="${SUPERAGENT_SERVICE_NAME:-superagent.service}"
health_url="${SUPERAGENT_HEALTH_URL:-http://127.0.0.1:8787/healthz}"
current_release_file="$install_root/current-release"

release_dir="${1:-}"
if [[ -z "$release_dir" && -f "$current_release_file" ]]; then
  current_release_dir="$(<"$current_release_file")"
  release_dir="$current_release_dir/previous"
else
  release_dir="${release_dir:-}"
fi

if [[ -d "$release_dir" ]]; then
  release_binary="$release_dir/superagent-server"
else
  release_binary="$release_dir"
fi

if [[ ! -f "$release_binary" || ! -x "$release_binary" ]]; then
  echo "usage: $0 /opt/superagent/releases/TIMESTAMP/previous-or-binary" >&2
  echo "rollback binary is missing or not executable: $release_binary" >&2
  exit 2
fi

umask 077
mkdir -p "$(dirname "$binary_path")"
install -m 0755 "$release_binary" "$binary_path.new.rollback"
mv -f "$binary_path.new.rollback" "$binary_path"

if command -v systemctl >/dev/null 2>&1 && systemctl is-active --quiet "$service_name"; then
  systemctl restart "$service_name"
fi

healthy=0
for _ in $(seq 1 30); do
  if command -v curl >/dev/null 2>&1 && curl --fail --silent --show-error --max-time 2 "$health_url" >/dev/null; then
    healthy=1
    break
  fi
  sleep 1
done

if [[ "$healthy" != 1 ]]; then
  echo "rollback binary installed but health check failed: $health_url" >&2
  exit 1
fi

printf 'rollback complete using %s\n' "$release_binary"
