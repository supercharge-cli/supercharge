#!/usr/bin/env bash
set -euo pipefail

runtime_dir="${SUPERAGENT_RUNTIME_DIR:-/var/lib/superagent}"
archive="${1:-}"

if [[ -z "$archive" || ! -f "$archive" ]]; then
  echo "usage: $0 BACKUP.tar.gz" >&2
  exit 2
fi

allowed='^(jobs|memory|gateway-state|controls|schedules|matrix-sync|e2b-sessions)\.json$'
while IFS= read -r entry; do
  if [[ ! "$entry" =~ $allowed ]]; then
    echo "backup contains an unsupported or unsafe entry: $entry" >&2
    exit 1
  fi
done < <(tar -tzf "$archive" | sed 's#/$##')

umask 077
mkdir -p "$runtime_dir"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
tar -xzf "$archive" -C "$tmp_dir"
for file in jobs.json memory.json gateway-state.json controls.json schedules.json matrix-sync.json e2b-sessions.json; do
  if [[ -f "$tmp_dir/$file" ]]; then
    install -m 600 "$tmp_dir/$file" "$runtime_dir/$file"
  fi
done
printf 'restored durable runtime state into %s\n' "$runtime_dir"
