#!/usr/bin/env bash
set -euo pipefail

e2b=0
case "${SUPERCHARGE_RUNTIME_MODE:-}${SUPERAGENT_RUNTIME_MODE:-}${SUPERCHARGE_RUNTIME:-}" in
  *[eE]2[bB]*) e2b=1 ;;
esac
if [ -n "${E2B_SANDBOX_ID:-}" ] || [ -f /run/e2b/.E2B_SANDBOX_ID ] || [ -f /run/e2b/E2B_SANDBOX_ID ]; then
  e2b=1
fi
[ "$e2b" = 1 ] || exit 0
export SUPERCHARGE_RUNTIME_MODE=e2b
export DISPLAY=:99
export XDG_CURRENT_DESKTOP=XFCE
export XDG_SESSION_TYPE=x11
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/supercharge-runtime-$(id -u)}"

runtime_dir=/tmp/supercharge-browser
base_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
profile_dir="${SUPERAGENT_BROWSER_PROFILE_DIR:-/home/user/.supercharge/browser-profile}"
mkdir -p "$runtime_dir" "$profile_dir" "$XDG_RUNTIME_DIR" "${SUPERAGENT_MEDIA_ROOT:-/home/user/workspace/.superagent-media}"
chmod 700 "$runtime_dir" "$XDG_RUNTIME_DIR"
exec 9>"$runtime_dir/start.lock"
flock -w 30 9

alive() {
  local file="$runtime_dir/$1.pid" pid
  [ -f "$file" ] || return 1
  read -r pid <"$file"
  [[ "$pid" =~ ^[0-9]+$ ]] && [ -O "/proc/$pid" ] && kill -0 "$pid" 2>/dev/null && [[ "$(ps -o stat= -p "$pid")" != Z* ]]
}

start_service() {
  local name="$1"; shift
  if alive "$name"; then return; fi
  "$@" 9>&- >"$runtime_dir/$name.log" 2>&1 &
  printf '%s\n' "$!" >"$runtime_dir/$name.pid"
}

if [ -f "$runtime_dir/Xauthority" ]; then export XAUTHORITY="$runtime_dir/Xauthority"; fi
if ! xdpyinfo -display :99 >/dev/null 2>&1; then
  if [ -e /tmp/.X99-lock ] || [ -S /tmp/.X11-unix/X99 ]; then
    printf '%s\n' 'Display :99 exists but is not accessible; refusing to delete X locks or restart a live desktop.' >&2
    exit 1
  fi
  export XAUTHORITY="$runtime_dir/Xauthority"
  touch "$XAUTHORITY"; chmod 600 "$XAUTHORITY"
  xauth -f "$XAUTHORITY" add :99 . "$(mcookie)"
  start_service xvfb Xvfb :99 -screen 0 "${SUPERAGENT_BROWSER_DISPLAY:-1280x800x24}" -nolisten tcp -auth "$XAUTHORITY"
  for _ in $(seq 1 100); do
    xdpyinfo -display :99 >/dev/null 2>&1 && break
    sleep 0.1
  done
  xdpyinfo -display :99 >/dev/null
fi
xset s off -dpms >/dev/null 2>&1 || true

export DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime_dir/bus"
if ! dbus-send --session --dest=org.freedesktop.DBus --type=method_call --print-reply / org.freedesktop.DBus.ListNames >/dev/null 2>&1; then
  if alive dbus; then printf '%s\n' 'The existing desktop bus is not ready; refusing to replace it.' >&2; exit 1; fi
  dbus-daemon --session --fork --address="$DBUS_SESSION_BUS_ADDRESS" --print-pid=1 >"$runtime_dir/dbus.pid" 9>&-
fi
start_service xfce xfce4-session

if ! curl --max-time 2 -fsS http://127.0.0.1:9222/json/version >/dev/null 2>&1; then
  if alive chromium; then printf '%s\n' 'The existing Chromium is not ready; preserving its process and profile.' >&2; exit 1; fi
  node "$base_dir/launch-browser.mjs" "$runtime_dir/chromium.pid" 9>&- >>"$runtime_dir/chromium.log" 2>&1
  for _ in $(seq 1 100); do
    curl --max-time 1 -fsS http://127.0.0.1:9222/json/version >/dev/null 2>&1 && break
    sleep 0.1
  done
  curl --max-time 2 -fsS http://127.0.0.1:9222/json/version >/dev/null
fi

if ! timeout 1 bash -c 'exec 3<>/dev/tcp/127.0.0.1/5900' 2>/dev/null; then
  start_service x11vnc x11vnc -display :99 -localhost -shared -forever -rfbport 5900 -nopw -xkb
fi
if ! curl --max-time 2 -fsS http://127.0.0.1:6080/vnc.html >/dev/null 2>&1; then
  start_service novnc websockify --web=/usr/share/novnc 127.0.0.1:6080 127.0.0.1:5900
fi
port="${SUPERAGENT_BROWSER_SIDECAR_PORT:-9224}"
if ! curl --max-time 2 -fsS "http://127.0.0.1:$port/health" >/dev/null 2>&1; then
  # Keep the historical sidecar PID path for updater and daemon compatibility.
  if [ -f "$runtime_dir/pid" ]; then
    read -r pid <"$runtime_dir/pid"
    if [[ "$pid" =~ ^[0-9]+$ ]] && [ -O "/proc/$pid" ] && kill -0 "$pid" 2>/dev/null && [[ "$(ps -o stat= -p "$pid")" != Z* ]]; then
      printf '%s\n' 'Sidecar process is starting or unhealthy; refusing a duplicate.' >&2
      exit 1
    fi
  fi
  node "$base_dir/server.mjs" 9>&- >"$runtime_dir/sidecar.log" 2>&1 &
  printf '%s\n' "$!" >"$runtime_dir/pid"
fi
for _ in $(seq 1 100); do
  curl --max-time 1 -fsS "http://127.0.0.1:$port/health" >/dev/null 2>&1 && break
  sleep 0.1
 done
curl --max-time 2 -fsS "http://127.0.0.1:$port/health" >/dev/null
