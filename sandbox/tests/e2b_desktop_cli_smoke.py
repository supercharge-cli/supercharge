#!/usr/bin/env python3
"""Exercise the real CLI against a local fake model; never calls a cloud model.

Verifies the desktop tools are advertised only when the runtime explicitly
enables them, and that ordinary E2B browser tools stay present either way.
"""
import argparse
import http.server
import json
import os
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time
import uuid


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--cli', default='target/debug/supercharge-pager')
    args = parser.parse_args()
    cli = str(Path(args.cli).resolve())
    requests = []
    tool_names = []
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args): pass
        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers.get('content-length','0'))))
            requests.append(body)
            tool_names.extend(tool.get('function',{}).get('name') for tool in body.get('tools',[]))
            self.send_response(200)
            self.send_header('content-type','application/json');self.end_headers()
            self.wfile.write(json.dumps({'id':'smoke','object':'chat.completion','created':1,'model':'smoke','choices':[{'index':0,'message':{'role':'assistant','content':'Desktop tool advertisement check completed.'},'finish_reason':'stop'}]}).encode())
    server = http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    base = f'http://127.0.0.1:{server.server_port}/v1'
    try:
        for desktop_enabled in (False, True):
            requests.clear(); tool_names.clear()
            with tempfile.TemporaryDirectory(prefix='vectra-desktop-cli-') as tmp:
                root=Path(tmp);session=str(uuid.uuid4());home=root/'cli';home.mkdir()
                (home/'config.toml').write_text(f'''[model.smoke]
model="smoke"
name="Local smoke"
base_url="{base}"
api_base_url="{base}"
env_key="SMOKE_API_KEY"
api_backend="chat_completions"
agent_type="supercharge-e2b"
context_window=128000
[agent]
name="supercharge-e2b"
''')
                env={key:value for key,value in os.environ.items() if not any(word in key for word in ('TOKEN','KEY','SECRET','SUPERCHARGE','SUPERAGENT','VECTRA'))}
                env.update({'HOME':tmp,'SUPERCHARGE_HOME':str(home),'SUPERCHARGE_RUNTIME_MODE':'e2b','SUPERCHARGE_AGENT':'supercharge-e2b',
                    'SUPERCHARGE_DISABLE_AUTOUPDATER':'1','SMOKE_API_KEY':'local-fake-key',
                    'OTEL_SDK_DISABLED':'true','SUPERCHARGE_API_BASE_URL':base,'SUPERCHARGE_CLI_CHAT_PROXY_BASE_URL':base})
                if desktop_enabled:
                    env['SUPERCHARGE_DESKTOP_PROTOCOL']='1'
                log=(root/'stderr').open('w')
                proc=subprocess.Popen([cli,'--no-auto-update','agent','--no-leader','--always-approve','--model','smoke',
                    '--supercharge-api-base-url',base,'--cli-chat-proxy-base-url',base,'stdio'],cwd=root,env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=log,text=True,start_new_session=True)
                output=queue.Queue()
                def read():
                    for line in proc.stdout:
                        try: output.put(json.loads(line))
                        except json.JSONDecodeError: pass
                threading.Thread(target=read,daemon=True).start()
                def call(id,method,params):
                    proc.stdin.write(json.dumps({'jsonrpc':'2.0','id':id,'method':method,'params':params})+'\n');proc.stdin.flush()
                    end=time.monotonic()+60
                    while time.monotonic()<end:
                        try:value=output.get(timeout=1)
                        except queue.Empty:
                            if proc.poll() is not None:raise AssertionError('CLI exited')
                            continue
                        if value.get('id')==id and ('result' in value or 'error' in value):
                            assert 'error' not in value,value
                            return value['result']
                        if 'id' in value and 'method' in value:
                            proc.stdin.write(json.dumps({'jsonrpc':'2.0','id':value['id'],'result':{'outcome':{'outcome':'cancelled'}}})+'\n');proc.stdin.flush()
                    raise AssertionError(f'{method} timed out')
                try:
                    call(1,'initialize',{'protocolVersion':1,'clientInfo':{'name':'desktop-smoke','version':'1'},'clientCapabilities':{},'_meta':{'clientType':'headless','startupHints':{'nonInteractive':True,'skipGitStatus':True,'skipProjectLayout':True}}})
                    call(2,'session/new',{'cwd':str(root),'mcpServers':[],'_meta':{'sessionId':session,'sessionKind':'headless','modelId':'smoke','yoloMode':True}})
                    call(3,'session/prompt',{'sessionId':session,'prompt':[{'type':'text','text':'List your available computer tools.'}],'_meta':{'promptId':str(uuid.uuid4()),'screenMode':'headless'}})
                    assert 'browser_snapshot' in tool_names, tool_names
                    if desktop_enabled:
                        assert 'desktop_observe' in tool_names and 'desktop_action' in tool_names, tool_names
                    else:
                        assert 'desktop_observe' not in tool_names and 'desktop_action' not in tool_names, tool_names
                    label = 'enabled' if desktop_enabled else 'disabled'
                    print(f'PASS: real CLI {"advertises" if desktop_enabled else "hides"} desktop tools when SUPERCHARGE_DESKTOP_PROTOCOL is {label}')
                except Exception:
                    log.flush();print((root/'stderr').read_text()[-10000:]);raise
                finally:
                    import signal
                    if proc.poll() is None:
                        os.killpg(proc.pid,signal.SIGTERM)
                        try:proc.wait(timeout=10)
                        except subprocess.TimeoutExpired:os.killpg(proc.pid,signal.SIGKILL);proc.wait()
                    log.close()
    finally:server.shutdown();server.server_close()

if __name__=='__main__':main()
