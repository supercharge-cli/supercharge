#!/usr/bin/env bash
# Publish Supercharge E2B binaries as a GitHub Release so existing sandboxes
# can `/update yes` / dashboard Update without a new template.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
template_dir="$repo_root/sandbox/e2b-agent-template"
bin_dir="$template_dir/bin"
tag="${SUPERCHARGE_E2B_UPDATE_TAG:?Set SUPERCHARGE_E2B_UPDATE_TAG to a new release version}"
release_repo="${SUPERCHARGE_E2B_UPDATE_REPO:-iotserver24/supercharge-releases}"

"$repo_root/sandbox/prepare-e2b-agent-assets.sh"
system_version="${tag#v}"
if [[ "$system_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  export SYSTEM_UPDATE_NOTES="Published with Supercharge ${tag}."
  "$template_dir/pack-system-bundle.sh" "$system_version"
fi

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

CLI-only updates use the existing feed. The desktop bundle uses sandbox-desktop.json
and requires explicit per-sandbox opt-in and desktop prerequisites; it is never installed on wake.
XibeCode instances are not affected.
EOF
)"

assets=(
  "$stage/supercharge-superagent-server-linux-x86_64"
  "$stage/supercharge-linux-x86_64"
  "$stage/e2b-manifest.json"
)
if [ -f "$template_dir/dist/sandbox-desktop.json" ]; then
  assets+=("$template_dir/dist/sandbox-desktop.json" "$template_dir/dist/sandbox-desktop-${system_version}.tar.gz")
fi
gh release create "$tag" --repo "$release_repo" \
  "${assets[@]}" \
  --title "Supercharge E2B ${tag}" \
  --notes "$notes"

echo
echo "Published GitHub release ${tag}"
echo "Sandboxes fetch: https://github.com/${release_repo}/releases/latest/download/"
echo "Then in an existing Supercharge VM: /update yes"
