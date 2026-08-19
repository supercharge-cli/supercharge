#!/usr/bin/env bash
set -euo pipefail

runtime_dir="${SUPERAGENT_RUNTIME_DIR:-/var/lib/superagent}"
archive="${1:-superagent-runtime-$(date -u +%Y%m%dT%H%M%SZ).tar.gz}"

if [[ ! -d "$runtime_dir" ]]; then
  echo "runtime directory does not exist: $runtime_dir" >&2
  exit 1
fi

files=()
for file in jobs.json memory.json gateway-state.json controls.json schedules.json matrix-sync.json e2b-sessions.json; do
  if [[ -f "$runtime_dir/$file" ]]; then
    files+=("$file")
  fi
done

if [[ ${#files[@]} -eq 0 ]]; then
  echo "no durable runtime state found in $runtime_dir" >&2
  exit 1
fi

umask 077
tar -czf "$archive" -C "$runtime_dir" "${files[@]}"
printf 'created %s from %s (%s files)\n' "$archive" "$runtime_dir" "${#files[@]}"
