#!/usr/bin/env bash
set -euo pipefail

mkdir -p \
  /home/user/workspace \
  /home/user/.superagent \
  /home/user/.supercharge/state \
  /home/user/workspace/.superagent-media \
  /home/user/.supercharge/browser-profile

/opt/vectra/wake-http/start.sh || true

set -a
[ -f /home/user/.superagent/daemon.env ] && . /home/user/.superagent/daemon.env
[ -f /home/user/.superagent/gateway.env ] && . /home/user/.superagent/gateway.env
set +a

export SUPERAGENT_BIND="${SUPERAGENT_BIND:-0.0.0.0:8790}"
export SUPERAGENT_AGENT_BINARY="${SUPERAGENT_AGENT_BINARY:-/usr/local/bin/supercharge}"
export SUPERAGENT_DAEMON_WORKDIR="${SUPERAGENT_DAEMON_WORKDIR:-/home/user/workspace}"
export SUPERAGENT_SHARE_WORKSPACE_ROOT="${SUPERAGENT_SHARE_WORKSPACE_ROOT:-/home/user/workspace}"
export SUPERAGENT_MEDIA_ROOT="${SUPERAGENT_MEDIA_ROOT:-/home/user/workspace/.superagent-media}"
export SUPERAGENT_BROWSER_CONTROL_ENABLED="${SUPERAGENT_BROWSER_CONTROL_ENABLED:-true}"
export SUPERAGENT_WAKE_PORT="${SUPERAGENT_WAKE_PORT:-8788}"
export SUPERAGENT_DAEMON_COMMAND="${SUPERAGENT_DAEMON_COMMAND:-supercharge agent daemon --bind 0.0.0.0:8790}"
export SUPERCHARGE_RUNTIME_MODE="${SUPERCHARGE_RUNTIME_MODE:-e2b}"

/opt/vectra/browser-runtime/start.sh

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
