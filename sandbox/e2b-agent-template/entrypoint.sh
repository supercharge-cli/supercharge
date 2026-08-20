#!/usr/bin/env bash
set -euo pipefail

mkdir -p /var/lib/superagent /home/user/workspace/media

# The wake service is the E2B auto-resume doorbell and file-share endpoint.
/opt/superagent/wake-http/start.sh || true

# Keep the Rust daemon in the foreground so E2B reports the sandbox as healthy
# and an external supervisor can restart the sandbox if it exits.
exec /usr/local/bin/superagent-server
