#!/usr/bin/env bash
set -euo pipefail

mkdir -p \
  /home/user/workspace \
  /home/user/.superagent \
  /home/user/.supercharge/state \
  /home/user/workspace/.superagent-media \
  /home/user/.supercharge/browser-profile

# Sandbox-local capability for the isolated Plugin API controller. The daemon
# presents it on every controller invocation; the controller (root via the
# fixed sudoers allowlist) verifies it before any privileged action.
if [ ! -s /home/user/.superagent/plugin-control.capability ]; then
  umask 077
  head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' \
    > /home/user/.superagent/plugin-control.capability
  chmod 600 /home/user/.superagent/plugin-control.capability
fi
export SUPERAGENT_PLUGIN_CONTROL="${SUPERAGENT_PLUGIN_CONTROL:-/opt/vectra/plugin-api/control.mjs}"
export SUPERAGENT_PLUGIN_API_NODE="${SUPERAGENT_PLUGIN_API_NODE:-/opt/vectra/plugin-api/node/bin/node}"
export SUPERAGENT_PLUGIN_CONTROL_CAPABILITY_FILE="${SUPERAGENT_PLUGIN_CONTROL_CAPABILITY_FILE:-/home/user/.superagent/plugin-control.capability}"

/opt/vectra/wake-http/start.sh || true

set -a
[ -f /home/user/.superagent/daemon.env ] && . /home/user/.superagent/daemon.env
[ -f /home/user/.superagent/gateway.env ] && . /home/user/.superagent/gateway.env
set +a

if [ -n "${SUPERAGENT_INSTANCE_ID:-}" ]; then
  umask 077
  printf '%s\n' "$SUPERAGENT_INSTANCE_ID" > /home/user/.superagent/plugin-instance.id
  chmod 600 /home/user/.superagent/plugin-instance.id
fi

export SUPERAGENT_BIND="${SUPERAGENT_BIND:-0.0.0.0:8790}"
export SUPERAGENT_AGENT_BINARY="${SUPERAGENT_AGENT_BINARY:-/usr/local/bin/supercharge}"
export SUPERAGENT_DAEMON_WORKDIR="${SUPERAGENT_DAEMON_WORKDIR:-/home/user/workspace}"
export SUPERAGENT_SHARE_WORKSPACE_ROOT="${SUPERAGENT_SHARE_WORKSPACE_ROOT:-/home/user/workspace}"
export SUPERAGENT_MEDIA_ROOT="${SUPERAGENT_MEDIA_ROOT:-/home/user/workspace/.superagent-media}"
export SUPERAGENT_BROWSER_CONTROL_ENABLED="${SUPERAGENT_BROWSER_CONTROL_ENABLED:-true}"
export SUPERAGENT_WAKE_PORT="${SUPERAGENT_WAKE_PORT:-8788}"
export SUPERAGENT_DAEMON_COMMAND="${SUPERAGENT_DAEMON_COMMAND:-supercharge agent daemon --bind 0.0.0.0:8790}"
export SUPERCHARGE_RUNTIME_MODE="${SUPERCHARGE_RUNTIME_MODE:-e2b}"

/opt/vectra/browser-runtime/start.sh || true

PIDFILE=/tmp/supercharge-daemon.pid
LOGFILE=/tmp/supercharge-daemon.log

if [ -f "$PIDFILE" ]; then
  OLD="$(cat "$PIDFILE" 2>/dev/null || true)"
  if [ -n "$OLD" ]; then
    kill "$OLD" 2>/dev/null || true
    sleep 1
    kill -9 "$OLD" 2>/dev/null || true
  fi
  rm -f "$PIDFILE"
fi

"$SUPERAGENT_DAEMON_COMMAND" >>"$LOGFILE" 2>&1 &
echo $! >"$PIDFILE"
wait "$(cat "$PIDFILE")"
