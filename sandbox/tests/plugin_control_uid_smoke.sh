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
  catalog_request="$(printf '%s' "$request" | /opt/vectra/plugin-api/node/bin/node -e 'let s="";process.stdin.on("data",x=>s+=x).on("end",()=>{const r=JSON.parse(s);r.action="installCatalog";r.catalogId="vectra-echo";r.catalogSha256=process.argv[1];r.requestId="fixture-catalog-001";process.stdout.write(JSON.stringify(r))})' 'a0c6be6c2f292c1a1ecdcc55056147d1dd232b58c76522e77ad44016ee7c0072')"
  bad_request="${catalog_request/a0c6be6c2f292c1a1ecdcc55056147d1dd232b58c76522e77ad44016ee7c0072/0000000000000000000000000000000000000000000000000000000000000000}"
  output="$(invoke "$bad_request" || true)"
  printf '%s\n' "$output" | /opt/vectra/plugin-api/node/bin/node -e 'let s="";process.stdin.on("data",x=>s+=x).on("end",()=>{if(!JSON.parse(s).error?.includes("Catalog release is unavailable"))process.exit(1)})'
  output="$(invoke "$catalog_request")"
  printf '%s\n' "$output" | /opt/vectra/plugin-api/node/bin/node -e 'let s="";process.stdin.on("data",x=>s+=x).on("end",()=>{const r=JSON.parse(s);if(!r.ok||r.result.plugin?.id!=="vectra-echo"||r.result.plugin?.enabled!==false||r.result.plugin?.trusted!==false)process.exit(1)})'
  update_request="$(printf '%s' "$output" | /opt/vectra/plugin-api/node/bin/node -e 'let s="";process.stdin.on("data",x=>s+=x).on("end",()=>{const r=JSON.parse(process.argv[1]);r.action="pluginAction";r.pluginId=r.catalogId;r.requestId="fixture-update-001";r.pluginAction={action:"update",expectedRevision:JSON.parse(s).result.plugin.revision,source:{type:"archive",url:"https://unreviewed.example/plugin.tar.gz",sha256:"0".repeat(64)}};process.stdout.write(JSON.stringify(r))})' "$catalog_request")"
  bad_update="${update_request/a0c6be6c2f292c1a1ecdcc55056147d1dd232b58c76522e77ad44016ee7c0072/0000000000000000000000000000000000000000000000000000000000000000}"
  output="$(invoke "$bad_update" || true)"
  printf '%s\n' "$output" | /opt/vectra/plugin-api/node/bin/node -e 'let s="";process.stdin.on("data",x=>s+=x).on("end",()=>{if(!JSON.parse(s).error?.includes("Catalog release is unavailable"))process.exit(1)})'
  mismatched_update="${update_request/\"pluginId\":\"vectra-echo\"/\"pluginId\":\"other-plugin\"}"
  output="$(invoke "$mismatched_update" || true)"
  printf '%s\n' "$output" | /opt/vectra/plugin-api/node/bin/node -e 'let s="";process.stdin.on("data",x=>s+=x).on("end",()=>{if(!JSON.parse(s).error?.includes("Catalog update target mismatch"))process.exit(1)})'
  output="$(invoke "$update_request")"
  printf '%s\n' "$output" | /opt/vectra/plugin-api/node/bin/node -e 'let s="";process.stdin.on("data",x=>s+=x).on("end",()=>{const r=JSON.parse(s);if(!r.ok||r.result.plugin?.id!=="vectra-echo"||r.result.plugin?.revision!==2||r.result.plugin?.enabled!==false||r.result.plugin?.trusted!==false)process.exit(1)})'
  github_request="$(printf '%s' "$request" | /opt/vectra/plugin-api/node/bin/node -e 'let s="";process.stdin.on("data",x=>s+=x).on("end",()=>{const r=JSON.parse(s);r.action="installCatalog";r.catalogId="vectra-github";r.catalogSha256="15f78177f39a2955d4a8cd462b135ea3c7d764f2e59da5be226f6c1fc4d724e6";r.requestId="fixture-github-001";process.stdout.write(JSON.stringify(r))})')"
  output="$(invoke "$github_request")"
  printf '%s\n' "$output" | /opt/vectra/plugin-api/node/bin/node -e 'let s="";process.stdin.on("data",x=>s+=x).on("end",()=>{const p=JSON.parse(s).result?.plugin;if(p?.id!=="vectra-github"||p.enabled||p.trusted||p.activeDigest!=="bef5d2b97bad049ea95e1a05170fc9a91fcabc1585b9b3c5896e86cc2a30b5e9")process.exit(1)})'
  configure_request="$(printf '%s' "$output" | /opt/vectra/plugin-api/node/bin/node -e 'let s="";process.stdin.on("data",x=>s+=x).on("end",()=>{const r=JSON.parse(process.argv[1]);r.action="pluginAction";r.pluginId="vectra-github";r.requestId="fixture-github-config-001";r.pluginAction={action:"configure",expectedRevision:JSON.parse(s).result.plugin.revision,values:{"github-token":"test-token-not-real"}};process.stdout.write(JSON.stringify(r))})' "$request")"
  output="$(invoke "$configure_request")"
  printf '%s\n' "$output" | /opt/vectra/plugin-api/node/bin/node -e 'let s="";process.stdin.on("data",x=>s+=x).on("end",()=>{const r=JSON.parse(s);if(!r.ok||JSON.stringify(r).includes("test-token-not-real")||!r.result?.plugin?.configuredKeys?.includes("github-token"))process.exit(1)})'
  output="$(invoke "$request")"
  printf '%s\n' "$output" | /opt/vectra/plugin-api/node/bin/node -e 'let s="";process.stdin.on("data",x=>s+=x).on("end",()=>{const r=JSON.parse(s).result;if(!r?.service?.allowedMcpOrigins?.includes("https://api.githubcopilot.com")||JSON.stringify(r).includes("test-token-not-real"))process.exit(1)})'
  echo "PASS: authorized Echo and GitHub catalog installs, write-only GitHub token, UID=$uid, isolated service UID=20000"
  exit 0
fi

for uid in 1000 1001; do
  docker run --rm --memory=1024m --cpus=1 --user root --entrypoint /bin/bash \
    -e "TEST_AGENT_UID=$uid" \
    --mount "type=bind,source=$controller,target=/opt/vectra/plugin-api/control.mjs,readonly" \
    --mount "type=bind,source=$repo_root/sandbox/tests/plugin_control_uid_smoke.sh,target=/tmp/plugin-control-smoke.sh,readonly" \
    --mount "type=bind,source=$repo_root/cloud-infra/apps/web/public/managed-mcp/catalog.json,target=/opt/vectra/plugin-api/catalog.json,readonly" \
    "$image" /tmp/plugin-control-smoke.sh --fixture
done
