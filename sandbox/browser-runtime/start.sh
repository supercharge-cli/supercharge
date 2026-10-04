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
bot_id="${SUPERAGENT_DESKTOP_BOT_ID:-main}"
if [[ ! "$bot_id" =~ ^[a-zA-Z0-9_-]{1,80}$ ]]; then
  printf '%s\n' 'Invalid desktop bot ID' >&2
  exit 1
fi
export XDG_CURRENT_DESKTOP=XFCE
export XDG_SESSION_TYPE=x11
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/supercharge-runtime-$(id -u)}"

root_dir=/tmp/supercharge-browser
base_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mkdir -p "$root_dir" "$XDG_RUNTIME_DIR"
chmod 700 "$root_dir" "$XDG_RUNTIME_DIR"
exec 9>"$root_dir/start.lock"
flock -w 30 9
runtime_dir="$root_dir"
display=99
cdp_port=9222
vnc_port=5900
novnc_port=6080
sidecar_port="${SUPERAGENT_BROWSER_SIDECAR_PORT:-9224}"
if [ "$bot_id" != main ]; then
  runtime_dir="$root_dir/bots/$bot_id"
  mkdir -p "$runtime_dir"
  chmod 700 "$runtime_dir"
  slot_file="$runtime_dir/slot"
  if [ -f "$slot_file" ]; then
    read -r slot <"$slot_file"
    [[ "$slot" =~ ^[0-9]+$ ]] && (( slot >= 1 && slot <= 300 )) || exit 1
  else
    for candidate in $(seq 1 300); do
      occupied=0
      for existing in "$root_dir"/bots/*/slot; do
        [ -f "$existing" ] || continue
        read -r assigned <"$existing"
        [ "$assigned" = "$candidate" ] && { occupied=1; break; }
      done
      [ "$occupied" = 0 ] || continue
      display=$((99 + candidate))
      [ ! -e "/tmp/.X${display}-lock" ] && [ ! -S "/tmp/.X11-unix/X${display}" ] || continue
      ports_free=1
      for candidate_port in $((12000 + candidate)) $((10000 + candidate)) $((11000 + candidate)) $((13000 + candidate)); do
        if timeout 1 bash -c "exec 3<>/dev/tcp/127.0.0.1/$candidate_port" 2>/dev/null; then ports_free=0; break; fi
      done
      [ "$ports_free" = 1 ] || continue
      slot="$candidate"
      printf '%s\n' "$slot" >"$slot_file"
      break
    done
    [ -f "$slot_file" ] || { printf '%s\n' 'No free desktop slot' >&2; exit 1; }
  fi
  display=$((99 + slot))
  cdp_port=$((12000 + slot))
  vnc_port=$((10000 + slot))
  novnc_port=$((11000 + slot))
  sidecar_port=$((13000 + slot))
fi
export DISPLAY=":$display"
export SUPERAGENT_DESKTOP_BOT_ID="$bot_id"
export SUPERAGENT_BROWSER_CDP_PORT="$cdp_port"
export SUPERAGENT_BROWSER_SIDECAR_PORT="$sidecar_port"
export SUPERAGENT_BROWSER_VNC_PORT="$vnc_port"
export SUPERAGENT_BROWSER_NOVNC_PORT="$novnc_port"
export SUPERAGENT_DESKTOP_RUNTIME_DIR="$runtime_dir"
export SUPERAGENT_BROWSER_PROFILE_DIR="${SUPERAGENT_BROWSER_PROFILE_DIR:-/home/user/.supercharge/browser-profile}"
if [ "$bot_id" != main ]; then export SUPERAGENT_BROWSER_PROFILE_DIR="/home/user/.supercharge/bot-browser-profiles/$bot_id"; fi
profile_dir="$SUPERAGENT_BROWSER_PROFILE_DIR"
mkdir -p "$profile_dir" "${SUPERAGENT_MEDIA_ROOT:-/home/user/workspace/.superagent-media}"
export SUPERAGENT_DESKTOP_HANDOFF_STATE="${SUPERAGENT_DESKTOP_HANDOFF_STATE:-/home/user/.supercharge/state/desktop-handoff.json}"
if [ "$bot_id" != main ]; then export SUPERAGENT_DESKTOP_HANDOFF_STATE="/home/user/.supercharge/state/desktop-handoff-$bot_id.json"; fi

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

export XAUTHORITY="$runtime_dir/Xauthority"
if [ "$bot_id" != main ]; then
  for service in chromium x11vnc novnc; do
    case "$service" in chromium) check_port="$cdp_port";; x11vnc) check_port="$vnc_port";; novnc) check_port="$novnc_port";; esac
    if timeout 1 bash -c "exec 3<>/dev/tcp/127.0.0.1/$check_port" 2>/dev/null && ! alive "$service"; then
      printf 'Desktop port %s is occupied by an unrelated process\n' "$check_port" >&2
      exit 1
    fi
  done
fi
if xdpyinfo -display "$DISPLAY" >/dev/null 2>&1; then
  if [ "$bot_id" != main ] && ! alive xvfb; then
    printf 'Display %s belongs to an unrelated process\n' "$DISPLAY" >&2
    exit 1
  fi
else
  if [ "$bot_id" != main ] && [ -f "/tmp/.X${display}-lock" ] && [ -O "/tmp/.X${display}-lock" ] && ! alive xvfb; then
    read -r lock_pid <"/tmp/.X${display}-lock" || true
    if [[ "$lock_pid" =~ ^[0-9]+$ ]] && ! kill -0 "$lock_pid" 2>/dev/null; then
      rm -f "/tmp/.X${display}-lock" "/tmp/.X11-unix/X${display}"
    fi
  fi
  if [ -e "/tmp/.X${display}-lock" ] || [ -S "/tmp/.X11-unix/X${display}" ]; then
    printf 'Display %s exists but is not accessible; preserving its X locks\n' "$DISPLAY" >&2
    exit 1
  fi
  export XAUTHORITY="$runtime_dir/Xauthority"
  touch "$XAUTHORITY"; chmod 600 "$XAUTHORITY"
  xauth -f "$XAUTHORITY" add "$DISPLAY" . "$(mcookie)"
  start_service xvfb Xvfb "$DISPLAY" -screen 0 "${SUPERAGENT_BROWSER_DISPLAY:-1280x800x24}" -nolisten tcp -auth "$XAUTHORITY"
  for _ in $(seq 1 100); do
    xdpyinfo -display "$DISPLAY" >/dev/null 2>&1 && break
    sleep 0.1
  done
  xdpyinfo -display "$DISPLAY" >/dev/null
fi
xset s off -dpms >/dev/null 2>&1 || true

export DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime_dir/bus"
if ! dbus-send --session --dest=org.freedesktop.DBus --type=method_call --print-reply / org.freedesktop.DBus.ListNames >/dev/null 2>&1; then
  if alive dbus; then printf '%s\n' 'The existing desktop bus is not ready; refusing to replace it.' >&2; exit 1; fi
  dbus-daemon --session --fork --address="$DBUS_SESSION_BUS_ADDRESS" --print-pid=1 >"$runtime_dir/dbus.pid" 9>&-
fi
start_service xfce xfce4-session

if ! curl --max-time 2 -fsS http://127.0.0.1:$cdp_port/json/version >/dev/null 2>&1; then
  if ! alive chromium; then
    node "$base_dir/launch-browser.mjs" "$runtime_dir/chromium.pid" 9>&- >>"$runtime_dir/chromium.log" 2>&1
  fi
  for _ in $(seq 1 300); do
    curl --max-time 1 -fsS http://127.0.0.1:$cdp_port/json/version >/dev/null 2>&1 && break
    sleep 0.1
  done
  curl --max-time 2 -fsS http://127.0.0.1:$cdp_port/json/version >/dev/null
fi

if ! timeout 1 bash -c "exec 3<>/dev/tcp/127.0.0.1/$vnc_port" 2>/dev/null; then
  start_service x11vnc x11vnc -display "$DISPLAY" -localhost -shared -forever -rfbport "$vnc_port" -nopw -xkb
fi
if ! curl --max-time 2 -fsS http://127.0.0.1:$novnc_port/vnc.html >/dev/null 2>&1; then
  start_service novnc websockify --web=/usr/share/novnc 127.0.0.1:$novnc_port 127.0.0.1:$vnc_port
fi
for _ in $(seq 1 100); do
  if timeout 1 bash -c "exec 3<>/dev/tcp/127.0.0.1/$vnc_port" 2>/dev/null && curl --max-time 1 -fsS "http://127.0.0.1:$novnc_port/vnc.html" >/dev/null 2>&1; then break; fi
  sleep 0.1
done
timeout 1 bash -c "exec 3<>/dev/tcp/127.0.0.1/$vnc_port"
curl --max-time 2 -fsS "http://127.0.0.1:$novnc_port/vnc.html" >/dev/null
port="$sidecar_port"
sidecar_ready() {
  curl --max-time 2 -fsS "http://127.0.0.1:$port/status" 2>/dev/null | node -e 'let data="";process.stdin.on("data",c=>data+=c);process.stdin.on("end",()=>{try{const s=JSON.parse(data);process.exit(s.desktopBotId===process.argv[1] && s.display===process.argv[2] ? 0 : 1)}catch{process.exit(1)}})' "$bot_id" "$DISPLAY"
}
if ! sidecar_ready; then
  # Keep the historical sidecar PID path for updater and daemon compatibility.
  if [ -f "$runtime_dir/pid" ]; then
    read -r pid <"$runtime_dir/pid"
    if [[ "$pid" =~ ^[0-9]+$ ]] && [ -O "/proc/$pid" ] && kill -0 "$pid" 2>/dev/null && [[ "$(ps -o stat= -p "$pid")" != Z* ]]; then
      for _ in $(seq 1 300); do
        if sidecar_ready; then exit 0; fi
        sleep 0.1
      done
      printf '%s\n' 'Existing sidecar did not become ready; preserving its process rather than starting a duplicate.' >&2
      exit 1
    fi
  fi
  node "$base_dir/server.mjs" 9>&- >"$runtime_dir/sidecar.log" 2>&1 &
  printf '%s\n' "$!" >"$runtime_dir/pid"
fi
for _ in $(seq 1 300); do
  sidecar_ready && break
  sleep 0.1
 done
sidecar_ready
