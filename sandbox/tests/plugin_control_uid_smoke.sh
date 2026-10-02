#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
image="${SUPERCHARGE_PLUGIN_TEST_IMAGE:-supercharge-plugin-e2b-scratch:1.3.26}"
controller="$repo_root/sandbox/e2b-agent-template/plugin-api-control.mjs"

if [ "${1:-}" = "--fixture" ]; then
  uid="${TEST_AGENT_UID:?}"
  if [ "$uid" = 1001 ]; then
    groupmod -g 1001 ubuntu
    usermod -u 1001 -g 1001 user
    chown -R 1001:1001 /home/user /var/lib/superagent /opt/vectra/wake-http /opt/vectra/browser-runtime
  fi
  [ "$(id -u user)" = "$uid" ]
  install -d -o user -g "$(id -gn user)" -m 0700 /home/user/.superagent
  install -d -o user -g "$(id -gn user)" -m 0700 /home/user/workspace
  install -d -m 0755 /run/e2b
  printf '%s\n' fixture-sandbox > /run/e2b/.E2B_SANDBOX_ID
  printf '%s\n' fixture-instance > /home/user/.superagent/plugin-instance.id
  printf '%064d\n' 1 > /home/user/.superagent/plugin-control.capability
  chown user:"$(id -gn user)" /home/user/.superagent/plugin-*.id /home/user/.superagent/plugin-control.capability
  chmod 0600 /home/user/.superagent/plugin-instance.id /home/user/.superagent/plugin-control.capability

  request='{"action":"status","instanceId":"fixture-instance","sandboxId":"fixture-sandbox","capability":"0000000000000000000000000000000000000000000000000000000000000001","bot":{"id":"fixture-bot","revision":1,"defaultFolder":"/home/user/workspace"}}'
  state=/var/lib/superagent/plugin-control/bots/fixture-bot/control.json
  invoke() { printf '%s' "$1" | runuser -u user -- sudo -n /opt/vectra/plugin-api/node/bin/node /opt/vectra/plugin-api/control.mjs; }
  denied() {
    local output
    output="$(invoke "$1" || true)"
    printf '%s\n' "$output" | /opt/vectra/plugin-api/node/bin/node -e 'let input="";process.stdin.on("data",x=>input+=x).on("end",()=>{if(JSON.parse(input).ok!==false)process.exit(1)})'
    test ! -e "$state"
  }
  denied "${request/fixture-instance/other-instance}"
  denied "${request/fixture-sandbox/other-sandbox}"
  denied "${request/0000000000000000000000000000000000000000000000000000000000000001/0000000000000000000000000000000000000000000000000000000000000002}"
  output="$(printf '%s' "$request" | env SUDO_USER=user SUDO_UID=999 /opt/vectra/plugin-api/node/bin/node /opt/vectra/plugin-api/control.mjs || true)"
  printf '%s\n' "$output" | /opt/vectra/plugin-api/node/bin/node -e 'let input="";process.stdin.on("data",x=>input+=x).on("end",()=>{if(!JSON.parse(input).error.includes("sandbox daemon account"))process.exit(1)})'
  test ! -e "$state"
  mv /home/user/.superagent/plugin-instance.id /home/user/.superagent/plugin-instance.id.test
  denied "$request"
  mv /home/user/.superagent/plugin-instance.id.test /home/user/.superagent/plugin-instance.id

  output="$(invoke "$request")"
  printf '%s\n' "$output" | /opt/vectra/plugin-api/node/bin/node -e 'let input="";process.stdin.on("data",x=>input+=x).on("end",()=>{const result=JSON.parse(input);if(!result.ok||result.result.agentUid!==Number(process.env.TEST_AGENT_UID)||!result.result.service.running)process.exit(1)})'
  output="$(invoke "$request")"
  printf '%s\n' "$output" | /opt/vectra/plugin-api/node/bin/node -e 'let input="";process.stdin.on("data",x=>input+=x).on("end",()=>{if(!JSON.parse(input).ok)process.exit(1)})'
  /opt/vectra/plugin-api/node/bin/node -e 'const fs=require("fs");const state=JSON.parse(fs.readFileSync(process.argv[1]));if(state.agentUid!==Number(process.env.TEST_AGENT_UID)||state.serviceUid!==20000||state.instanceId!=="fixture-instance"||state.sandboxId!=="fixture-sandbox")process.exit(1)' "$state"
  test "$(stat -c '%u' /var/lib/superagent/plugin-control/services/fixture-bot/config.json)" = 20000
  test "$(stat -c '%a' "$state")" = 600
  getfacl -cp /home/user | grep -qx 'user:20000:--x'
  test "$(stat -c '%u' /var/lib/superagent/plugin-control/services/fixture-bot/data)" = 20000
  echo "PASS: authorized controller status, UID=$uid, isolated service UID=20000, rejected mismatched authority"
  exit 0
fi

for uid in 1000 1001; do
  docker run --rm --memory=1024m --cpus=1 --user root --entrypoint /bin/bash \
    -e "TEST_AGENT_UID=$uid" \
    --mount "type=bind,source=$controller,target=/opt/vectra/plugin-api/control.mjs,readonly" \
    --mount "type=bind,source=$repo_root/sandbox/tests/plugin_control_uid_smoke.sh,target=/tmp/plugin-control-smoke.sh,readonly" \
    "$image" /tmp/plugin-control-smoke.sh --fixture
done
