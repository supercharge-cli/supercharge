#!/usr/bin/env bash
set -euo pipefail

e2b=0
case "${SUPERCHARGE_RUNTIME_MODE:-}${SUPERAGENT_RUNTIME_MODE:-}${SUPERCHARGE_RUNTIME:-}" in
  *[eE]2[bB]*) e2b=1 ;;
esac
if [ -n "${E2B_SANDBOX_ID:-}" ] || [ -f /run/e2b/.E2B_SANDBOX_ID ] || [ -f /run/e2b/E2B_SANDBOX_ID ]; then
  e2b=1
fi
if [ "$e2b" != 1 ]; then
  exit 0
fi
export SUPERCHARGE_RUNTIME_MODE=e2b

runtime_dir=/tmp/supercharge-browser
mkdir -p "$runtime_dir" "${SUPERAGENT_BROWSER_PROFILE_DIR:-/home/user/.supercharge/browser-profile}" "${SUPERAGENT_MEDIA_ROOT:-/home/user/workspace/.superagent-media}"

if [ -f "$runtime_dir/pid" ] && kill -0 "$(cat "$runtime_dir/pid")" 2>/dev/null; then
  exit 0
fi

rm -f /tmp/.X99-lock
Xvfb :99 -screen 0 "${SUPERAGENT_BROWSER_DISPLAY:-1280x800x24}" -nolisten tcp >"$runtime_dir/xvfb.log" 2>&1 &
export DISPLAY=:99

for _ in $(seq 1 30); do
  [ -S /tmp/.X11-unix/X99 ] && break
  sleep 0.1
done

x11vnc -display :99 -localhost -shared -forever -rfbport 5900 -nopw >"$runtime_dir/x11vnc.log" 2>&1 &
websockify --web=/usr/share/novnc 127.0.0.1:6080 127.0.0.1:5900 >"$runtime_dir/novnc.log" 2>&1 &
node /opt/vectra/browser-runtime/server.mjs >"$runtime_dir/sidecar.log" 2>&1 &
echo $! >"$runtime_dir/pid"
