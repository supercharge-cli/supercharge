#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
image="${SUPERCHARGE_PLUGIN_TEST_IMAGE:-supercharge-plugin-e2b-scratch:1.3.29}"

if [ "${1:-}" = "--fixture" ]; then
  install -d -o user -g "$(id -gn user)" -m 0700 /home/user/.superagent /home/user/workspace
  install -d -m 0755 /run/e2b
  printf '%s\n' fixture-sandbox > /run/e2b/.E2B_SANDBOX_ID
  printf '%s\n' fixture-instance > /home/user/.superagent/plugin-instance.id
  printf '%064d\n' 1 > /home/user/.superagent/plugin-control.capability
  chown user:"$(id -gn user)" /home/user/.superagent/plugin-instance.id /home/user/.superagent/plugin-control.capability
  chmod 0600 /home/user/.superagent/plugin-instance.id /home/user/.superagent/plugin-control.capability
  python3 - <<'PY'
import json, os, pathlib, subprocess, uuid

node = '/opt/vectra/plugin-api/node/bin/node'
controller = '/opt/vectra/plugin-api/control.mjs'
base = {
    'instanceId': 'fixture-instance', 'sandboxId': 'fixture-sandbox',
    'capability': '0' * 63 + '1',
    'bot': {'id': 'fixture-bot', 'revision': 1, 'defaultFolder': '/home/user/workspace'},
}
def call(action, **kwargs):
    payload = json.dumps({**base, 'action': action, **kwargs}).encode()
    result = subprocess.run(['runuser', '-u', 'user', '--', 'sudo', '-n', node, controller],
                            input=payload, capture_output=True, timeout=150)
    data = json.loads(result.stdout)
    if result.returncode or not data.get('ok'):
        raise RuntimeError(f'{action} failed: {data.get("error", result.stderr.decode()[:200])}')
    return data['result']

plugin = call('installSkillMd', owner='anthropic', name='pdf', requestId=str(uuid.uuid4()))['plugin']
assert plugin['id'].startswith('skillmd-') and not plugin['enabled'] and not plugin['trusted']
root = pathlib.Path('/var/lib/superagent/plugin-control/skillmd/fixture-bot')
assert not (root / plugin['id']).exists()
for action, params in [('trust', {'permissions': []}), ('enable', {})]:
    plugin = call('pluginAction', pluginId=plugin['id'], requestId=str(uuid.uuid4()),
                  pluginAction={'action': action, 'expectedRevision': plugin['revision'], **params})['plugin']
assert plugin['enabled'] and plugin['trusted']
publication = root / plugin['id']
assert (publication / 'SKILL.md').is_file()
assert (publication / 'scripts/convert_pdf_to_images.py').is_file()
assert publication.stat().st_uid == 0 and publication.stat().st_mode & 0o222 == 0
assert plugin['id'] in call('syncSkillMd')['skillMdIds']
plugin = call('pluginAction', pluginId=plugin['id'], requestId=str(uuid.uuid4()),
              pluginAction={'action': 'disable', 'expectedRevision': plugin['revision']})['plugin']
assert not publication.exists() and plugin['id'] not in call('syncSkillMd')['skillMdIds']
call('pluginAction', pluginId=plugin['id'], requestId=str(uuid.uuid4()),
     pluginAction={'action': 'uninstall', 'expectedRevision': plugin['revision']})
assert not publication.exists()
print('PASS: SkillMD ZIP installed disabled, explicitly approved, published with support files, then removed')
PY
  exit 0
fi

docker run --rm --memory=1536m --cpus=2 --user root --entrypoint /bin/bash \
  --mount "type=bind,source=$repo_root/sandbox/e2b-agent-template/plugin-api-control.mjs,target=/opt/vectra/plugin-api/control.mjs,readonly" \
  --mount "type=bind,source=$repo_root/sandbox/tests/skillmd_controller_smoke.sh,target=/tmp/skillmd-controller-smoke.sh,readonly" \
  "$image" /tmp/skillmd-controller-smoke.sh --fixture
