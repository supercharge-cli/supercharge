#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
template_dir="$repo_root/sandbox/e2b-agent-template"
if [ -z "${E2B_API_KEY:-}" ] && [ -f "$repo_root/.env.runtime" ]; then
  set -a
  # shellcheck disable=SC1090
  source "$repo_root/.env.runtime"
  set +a
fi

if [ -z "${E2B_API_KEY:-}" ] && [ -f "$repo_root/cloud-infra/apps/gateway/.env" ]; then
  set -a
  # shellcheck disable=SC1090
  source "$repo_root/cloud-infra/apps/gateway/.env"
  set +a
fi

template_name="${SUPERCHARGE_E2B_TEMPLATE:-supercharge-full-sandbox}"
cpu_count="${SUPERCHARGE_E2B_CPU_COUNT:-16}"
memory_mb="${SUPERCHARGE_E2B_MEMORY_MB:-16384}"
e2b_cli_version="${SUPERCHARGE_E2B_CLI_VERSION:-2.16.1}"
release_repo="${SUPERCHARGE_E2B_UPDATE_REPO:-iotserver24/supercharge-releases}"
system_version="$(python3 - "$repo_root/crates/codegen/supercharge-pager-bin/Cargo.toml" <<'PY'
import pathlib, re, sys
match = re.search(r'^version = "(\d+\.\d+\.\d+)"$', pathlib.Path(sys.argv[1]).read_text(), re.M)
if not match:
    raise SystemExit('Cannot determine Supercharge release version')
print(match.group(1))
PY
)"
release_tag="v${system_version}"

if [ -z "${E2B_API_KEY:-}" ]; then
  echo "E2B_API_KEY is required to publish the template" >&2
  exit 1
fi
if ! command -v gh >/dev/null 2>&1 || ! gh release view "$release_tag" --repo "$release_repo" >/dev/null 2>&1; then
  echo "Release $release_tag is not available in $release_repo; refusing to publish a template or upload assets to another version." >&2
  exit 1
fi

"$repo_root/sandbox/prepare-e2b-agent-assets.sh"

cd "$template_dir"
e2b_cli() {
  if command -v e2b >/dev/null 2>&1; then
    e2b "$@"
  else
    npm exec --yes \
      --package="@e2b/cli@${e2b_cli_version}" \
      --package=glob \
      e2b -- "$@"
  fi
}

e2b_cli template create "$template_name" \
  -d Dockerfile \
  --cpu-count "$cpu_count" \
  --memory-mb "$memory_mb"

e2b_cli template publish "$template_name" -y

export SYSTEM_UPDATE_NOTES="Published with E2B template ${template_name}."
"$template_dir/pack-system-bundle.sh" "$system_version"

gh release upload "$release_tag" --repo "$release_repo" --clobber \
  "$template_dir/dist/sandbox-desktop.json" \
  "$template_dir/dist/sandbox-desktop-${system_version}.tar.gz"

cat <<EOF

Published E2B template alias: ${template_name}
System bundle ${system_version} uploaded to ${release_repo} release ${release_tag}.

New sandboxes get this template. Existing sandboxes are NOT migrated on wake.
The desktop bundle uses a separate opt-in feed and requires an idle, explicitly prepared sandbox.
Workspace files and the existing browser session stay on the same sandbox.

  SUPERCHARGE_E2B_TEMPLATE=${template_name}

EOF
