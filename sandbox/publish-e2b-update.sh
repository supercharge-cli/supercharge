#!/usr/bin/env bash
# Publish Supercharge E2B binaries as a GitHub Release so existing sandboxes
# can `/update yes` / dashboard Update without a new template.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
template_dir="$repo_root/sandbox/e2b-agent-template"
bin_dir="$template_dir/bin"
tag="${SUPERCHARGE_E2B_UPDATE_TAG:-1.1.0}"

"$repo_root/sandbox/prepare-e2b-agent-assets.sh"

if [ ! -x "$bin_dir/supercharge-superagent-server" ] || [ ! -x "$bin_dir/supercharge" ]; then
  echo "Missing E2B binaries in $bin_dir" >&2
  exit 1
fi

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
cp "$bin_dir/supercharge-superagent-server" "$stage/supercharge-superagent-server-linux-x86_64"
cp "$bin_dir/supercharge" "$stage/supercharge-linux-x86_64"
cat > "$stage/e2b-manifest.json" <<EOF
{"version":"${tag}","daemon":"supercharge-superagent-server-linux-x86_64","cli":"supercharge-linux-x86_64"}
EOF

if ! command -v gh >/dev/null 2>&1; then
  echo "gh CLI is required to publish the update feed" >&2
  exit 1
fi

notes="$(cat <<EOF
Supercharge E2B hot-update (no new template).

Existing Vectra Supercharge sandboxes can run /update yes or dashboard Update.
XibeCode instances are not affected.
EOF
)"

gh release create "$tag" \
  "$stage/supercharge-superagent-server-linux-x86_64" \
  "$stage/supercharge-linux-x86_64" \
  "$stage/e2b-manifest.json" \
  --title "Supercharge E2B ${tag}" \
  --notes "$notes"

echo
echo "Published GitHub release ${tag}"
echo "Sandboxes fetch: https://github.com/iotserver24/supercharge/releases/latest/download/"
echo "Then in an existing Supercharge VM: /update yes"
