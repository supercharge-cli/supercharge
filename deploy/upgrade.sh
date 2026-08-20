#!/usr/bin/env bash
set -euo pipefail

release_binary="${1:-}"
if [[ -z "$release_binary" || ! -f "$release_binary" || ! -x "$release_binary" ]]; then
  echo "usage: $0 /path/to/superagent-server" >&2
  echo "the release binary must exist and be executable" >&2
  exit 2
fi

install_root="${SUPERAGENT_INSTALL_ROOT:-/opt/superagent}"
binary_path="${SUPERAGENT_BINARY_PATH:-$install_root/bin/superagent-server}"
runtime_dir="${SUPERAGENT_RUNTIME_DIR:-/var/lib/superagent}"
service_name="${SUPERAGENT_SERVICE_NAME:-superagent.service}"
health_url="${SUPERAGENT_HEALTH_URL:-http://127.0.0.1:8787/healthz}"
release_root="$install_root/releases"
timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
release_dir="$release_root/$timestamp"
backup_script="$(dirname "$0")/backup-runtime.sh"

if [[ "$release_binary" == *"/"* && ! -e "$release_binary" ]]; then
  echo "release binary does not exist: $release_binary" >&2
  exit 1
fi

umask 077
mkdir -p "$install_root/bin" "$release_dir" "$runtime_dir/backups"

if [[ -x "$binary_path" ]]; then
  install -m 0755 "$binary_path" "$release_dir/previous"
fi
install -m 0755 "$release_binary" "$release_dir/superagent-server"

if [[ -x "$backup_script" && -d "$runtime_dir" ]]; then
  "$backup_script" "$runtime_dir/backups/pre-upgrade-$timestamp.tar.gz" || {
    echo "runtime backup failed; refusing to upgrade" >&2
    exit 1
  }
fi

new_binary="$binary_path.new.$timestamp"
install -m 0755 "$release_dir/superagent-server" "$new_binary"
mv -f "$new_binary" "$binary_path"
printf '%s\n' "$release_dir" > "$install_root/current-release"

restart_needed=0
if command -v systemctl >/dev/null 2>&1 && systemctl is-active --quiet "$service_name"; then
  restart_needed=1
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
  echo "new release failed health check; restoring previous binary" >&2
  if [[ -f "$release_dir/previous" ]]; then
    install -m 0755 "$release_dir/previous" "$binary_path"
    if [[ "$restart_needed" == 1 ]]; then
      systemctl restart "$service_name" || true
    fi
  else
    echo "no previous binary was available; manual recovery is required" >&2
  fi
  exit 1
fi

printf 'upgrade complete: %s\n' "$release_dir"
