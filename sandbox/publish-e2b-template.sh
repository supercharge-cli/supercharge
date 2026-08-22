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
cpu_count="${SUPERCHARGE_E2B_CPU_COUNT:-4}"
memory_mb="${SUPERCHARGE_E2B_MEMORY_MB:-8192}"

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
    pnpm dlx @e2b/cli@latest "$@"
  fi
}

e2b_cli template create "$template_name" \
  -d Dockerfile \
  --cpu-count "$cpu_count" \
  --memory-mb "$memory_mb"

e2b_cli template publish "$template_name" -y

cat <<EOF

Published E2B template alias: ${template_name}

Set this on the Vectra gateway before creating Supercharge instances:

  SUPERCHARGE_E2B_TEMPLATE=${template_name}

EOF
