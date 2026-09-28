#!/usr/bin/env bash
# Pack the non-CLI sandbox files (wake server, file server, browser helper)
# into a gzip archive the gateway can apply on an existing sandbox.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
VERSION="${1:?usage: pack-system-bundle.sh <version>}"
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "version must be MAJOR.MINOR.PATCH" >&2
  exit 1
fi

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
OUT="${ROOT}/dist"
mkdir -p "$OUT" "$STAGE/opt/vectra"

copy_tree() {
  local src="$1" dest="$2"
  if [[ ! -d "$src" ]]; then
    echo "missing $src" >&2
    exit 1
  fi
  mkdir -p "$dest"
  cp -a "$src"/. "$dest"/
}

copy_tree "$ROOT/wake-http" "$STAGE/opt/vectra/wake-http"
if [[ -d "$ROOT/browser-runtime" ]]; then
  copy_tree "$ROOT/browser-runtime" "$STAGE/opt/vectra/browser-runtime"
fi

python3 - "$STAGE" <<'PY'
import os, sys
root = sys.argv[1]
allowed = (
    "/opt/vectra/wake-http/",
    "/opt/vectra/browser-runtime/",
    "/opt/vectra/bin/",
    "/usr/local/lib/supercharge/",
)
for dirpath, _, names in os.walk(root):
    for name in names:
        full = os.path.join(dirpath, name)
        rel = "/" + os.path.relpath(full, root)
        if not any(rel.startswith(prefix) for prefix in allowed):
            raise SystemExit(f"refusing {rel}")
print("bundle paths ok")
PY

ARCHIVE="$OUT/sandbox-system-${VERSION}.tar.gz"
tar -C "$STAGE" -czf "$ARCHIVE" opt
NOTES="${SYSTEM_UPDATE_NOTES:-System files updated on this sandbox. Your workspace was kept.}"
python3 - "$OUT/sandbox-system.json" "$VERSION" "$NOTES" <<'PY'
import json, sys
path, version, notes = sys.argv[1:]
json.dump({
    "version": version,
    "notes": [notes],
    "archive": f"sandbox-system-{version}.tar.gz",
    "paths": ["/opt/vectra/wake-http/", "/opt/vectra/browser-runtime/"],
    "commands": [],
}, open(path, "w"), indent=2)
print(path)
PY
echo "$ARCHIVE"
