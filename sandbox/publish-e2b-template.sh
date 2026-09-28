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

if [ -z "${E2B_API_KEY:-}" ]; then
  echo "E2B_API_KEY is required to publish the template" >&2
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

release_repo="${SUPERCHARGE_E2B_UPDATE_REPO:-iotserver24/supercharge-releases}"
system_version="$(
  python3 - "$release_repo" "$repo_root" <<'PY'
import json, pathlib, sys, urllib.request
repo, root = sys.argv[1], pathlib.Path(sys.argv[2])
url = f"https://github.com/{repo}/releases/latest/download/sandbox-system.json"
try:
    with urllib.request.urlopen(url, timeout=15) as response:
        current = json.load(response)["version"].lstrip("v")
    major, minor, patch = (int(part) for part in current.split("."))
    print(f"{major}.{minor}.{patch + 1}")
except Exception:
    text = (root / "crates/codegen/supercharge-pager-bin/Cargo.toml").read_text()
    for line in text.splitlines():
        if line.startswith("version = "):
            print(line.split('"')[1])
            break
PY
)"

export SYSTEM_UPDATE_NOTES="Published with E2B template ${template_name}."
"$template_dir/pack-system-bundle.sh" "$system_version"

if ! command -v gh >/dev/null 2>&1; then
  echo "gh CLI is required to upload the system bundle for existing sandboxes" >&2
  exit 1
fi

release_tag="$(gh release view --repo "$release_repo" --json tagName --jq .tagName)"
gh release upload "$release_tag" --repo "$release_repo" --clobber \
  "$template_dir/dist/sandbox-system.json" \
  "$template_dir/dist/sandbox-system-${system_version}.tar.gz"

cat <<EOF

Published E2B template alias: ${template_name}
System bundle ${system_version} uploaded to ${release_repo} release ${release_tag}.

New sandboxes get this template. Existing sandboxes install the bundle on their next wake.
Workspace files stay on the same sandbox.

  SUPERCHARGE_E2B_TEMPLATE=${template_name}

EOF
