#!/usr/bin/env python3
"""Local E2B-daemon protocol smoke test. No E2B or model credentials are used."""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

FAKE_CLI = r'''#!/usr/bin/env python3
import json,os,sys,threading,time,pathlib,urllib.parse
session='';lock=threading.Lock();cancel=threading.Event();rules=''
def emit(value):
    with lock: print(json.dumps(value),flush=True)
def reply(obj,result): emit({'jsonrpc':'2.0','id':obj['id'],'result':result})
def prompt(obj,text):
    cancel.clear()
    if text.startswith('wait:'): cancel.wait(float(text.split(':')[1]))
    summary={'bot':os.environ.get('VECTRA_BOT_ID'),'cwd':os.getcwd(),'rules':rules,'mainCredential':bool(os.environ.get('VECTRA_BOT_CAPABILITY'))}
    message=json.dumps(summary)
    emit({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':session,'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':message}}}})
    reply(obj,{'stopReason':'cancelled' if cancel.is_set() else 'end_turn'})
for line in sys.stdin:
    obj=json.loads(line);method=obj.get('method');params=obj.get('params',{})
    if method=='initialize':
        rules=params.get('_meta',{}).get('rules',''); reply(obj,{'protocolVersion':1});continue
    if method in ('session/new','session/load'):
        session=params.get('sessionId') or params.get('_meta',{}).get('sessionId')
        folder=pathlib.Path(os.environ['SUPERCHARGE_HOME'])/'sessions'/urllib.parse.quote(os.getcwd(),safe='')/session
        folder.mkdir(parents=True,exist_ok=True)
        rules=params.get('_meta',{}).get('systemPromptOverride') or 'BASE CLI RULES\n'+rules
        (folder/'summary.json').write_text('{}')
        (folder/'system_prompt.txt').write_text(rules)
        reply(obj,{'sessionId':session});continue
    if method=='session/prompt':
        text=params['prompt'][0]['text'];threading.Thread(target=prompt,args=(obj,text),daemon=True).start();continue
    if method=='session/cancel': cancel.set();continue
    if 'id' in obj: reply(obj,{})
'''

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--daemon', default='target/debug/supercharge-superagent-server')
    args = parser.parse_args()
    daemon = Path(args.daemon).resolve()
    with tempfile.TemporaryDirectory(prefix='vectra-bots-smoke-') as temporary:
        root = Path(temporary)
        workspace = root/'workspace'; workspace.mkdir()
        fake = root/'fake-cli'; fake.write_text(FAKE_CLI); fake.chmod(0o700)
        sock=socket.socket();sock.bind(('127.0.0.1',0));port=sock.getsockname()[1];sock.close()
        env={key:value for key,value in os.environ.items() if not any(word in key for word in ('TOKEN','KEY','SECRET','SUPERAGENT','SUPERCHARGE','VECTRA'))}
        env.update({'SUPERCHARGE_RUNTIME_MODE':'e2b','SUPERAGENT_INSTANCE_ID':'smoke-instance',
            'SUPERAGENT_BIND':f'127.0.0.1:{port}','SUPERAGENT_API_TOKEN':'local-smoke-token',
            'SUPERAGENT_AGENT_BINARY':str(fake),'SUPERAGENT_SHARE_WORKSPACE_ROOT':str(workspace),
            'SUPERAGENT_DAEMON_WORKDIR':str(workspace),'SUPERAGENT_PROVIDER_CONFIG_PATH':str(root/'provider.toml'),
            'SUPERAGENT_PROVIDER':'custom','SUPERAGENT_PROVIDER_BASE_URL':'http://127.0.0.1:1',
            'SUPERAGENT_BROWSER_SIDECAR_PORT':'1'})
        for name in ('SESSION','BOT','BOT_QUEUE','MEMORY','CONTROL','JOB','SCHEDULE'):
            env[f'SUPERAGENT_{name}_STATE']=str(root/f'{name.lower()}.json')
        log=(root/'daemon.log').open('w')
        process=None
        base=f'http://127.0.0.1:{port}'
        def request(path,data=None):
            encoded=None if data is None else json.dumps(data).encode()
            req=urllib.request.Request(base+path,data=encoded,headers={'authorization':'Bearer local-smoke-token','content-type':'application/json'})
            with urllib.request.urlopen(req,timeout=20) as response: return json.load(response)
        def start():
            proc=subprocess.Popen([str(daemon)],cwd=root,env=env,stdout=log,stderr=log)
            for _ in range(150):
                if proc.poll() is not None: raise AssertionError('daemon exited')
                try: request('/health');return proc
                except (OSError,urllib.error.URLError): time.sleep(.04)
            raise AssertionError('daemon startup timed out')
        def wait_for(predicate,timeout=20):
            until=time.monotonic()+timeout
            while time.monotonic()<until:
                value=predicate()
                if value:return value
                time.sleep(.06)
            raise AssertionError('condition timed out')
        def conversation(bot):
            return request('/v1/conversations/new',{'botId':bot})['active']
        def messages(chat,role='assistant'):
            return [item for item in request('/v1/conversations/'+chat)['messages'] if item['role']==role]
        def send(bot,chat,text):return request('/v1/chat',{'botId':bot,'chatId':chat,'text':text})
        def sync(bots):return request('/v1/app/bots/sync',{'instanceId':'smoke-instance','site':'https://example.test','capability':'s'*48,'bots':bots})
        bots=[{'id':'main','name':'Main','icon':'🤖','instructions':'main rules','defaultFolder':str(workspace),'revision':1}]
        for bot in ('alpha','beta'):
            bots.append({'id':bot,'name':bot.title(),'icon':'🔎','instructions':f'{bot} instructions','defaultFolder':str(workspace/bot),'revision':1})
        try:
            process=start();sync(bots)
            main_chat=conversation('main');alpha=conversation('alpha');beta=conversation('beta')
            send('alpha',alpha,'wait:1.5');send('beta',beta,'wait:1.5')
            status=wait_for(lambda:(lambda status: status if len(status['activeBotChats'])==2 else None)(request('/v1/status')))
            assert set(status['activeBotChats'])=={alpha,beta}
            a=json.loads(wait_for(lambda:messages(alpha))[-1]['content']);b=json.loads(wait_for(lambda:messages(beta))[-1]['content'])
            assert a['bot']=='alpha' and a['cwd']==str(workspace/'alpha') and 'alpha instructions' in a['rules']
            assert b['bot']=='beta' and b['cwd']==str(workspace/'beta') and not b['mainCredential']
            wait_for(lambda:not request('/v1/status')['activeBotChats'])
            send('main',main_chat,'hello');m=json.loads(wait_for(lambda:messages(main_chat))[-1]['content']);assert m['mainCredential']
            wait_for(lambda:not request('/v1/status')['activeBotChats'])
            bots[1]['revision']=2;bots[1]['instructions']='changed alpha instructions';bots[1]['defaultFolder']=str(workspace/'alpha-new');sync(bots)
            send('alpha',alpha,'again');a=json.loads(wait_for(lambda:messages(alpha) if len(messages(alpha))>=2 else None)[-1]['content'])
            assert a['cwd']==str(workspace/'alpha') and 'changed alpha instructions' in a['rules'] and 'BASE CLI RULES' in a['rules']
            new_alpha=conversation('alpha');send('alpha',new_alpha,'new folder');assert json.loads(wait_for(lambda:messages(new_alpha))[-1]['content'])['cwd']==str(workspace/'alpha-new')
            wait_for(lambda:not request('/v1/status')['activeBotChats'])
            try:send('beta',alpha,'wrong bot');raise AssertionError('wrong bot accepted')
            except urllib.error.HTTPError as error:assert error.code==404
            send('alpha',alpha,'wait:4');send('beta',beta,'wait:4')
            wait_for(lambda:len(request('/v1/status')['activeBotChats'])==2)
            assert request('/v1/chat/stop',{'botId':'alpha','chatId':alpha})['ok']
            assert beta in request('/v1/status')['activeBotChats']
            request('/v1/app/maintenance',{'action':'drain'})
            try:send('main',main_chat,'blocked');raise AssertionError('maintenance accepted chat')
            except urllib.error.HTTPError as error:assert error.code==503
            request('/v1/app/maintenance',{'action':'interrupt'});assert not request('/v1/status')['activeBotChats']
            request('/v1/app/maintenance',{'action':'resume'})
            job={'jobId':'job-smoke-1','botId':'beta','chatId':'bot-beta','text':'delegate work'}
            request('/v1/app/bot-jobs',job)
            wait_for(lambda:request('/v1/app/bot-jobs/job-smoke-1')['status']=='succeeded')
            request('/v1/app/bot-jobs',job);assert len(messages('bot-beta'))==1
            send('alpha',alpha,'wait:20');wait_for(lambda:alpha in request('/v1/status')['activeBotChats'])
            process.kill();process.wait(timeout=10);process=start()
            assert messages(alpha,'error'), 'interruption missing from history'
            assert len(messages('bot-beta'))==1
            assert request('/v1/conversations?botId=beta')['conversations']
            print('PASS: concurrent bots, instructions/cwd, Main capability, targeting, cancellation, maintenance, delegation idempotency, restart recovery')
        except Exception:
            log.flush();print((root/'daemon.log').read_text()[-7000:]);raise
        finally:
            if process and process.poll() is None:process.terminate();process.wait(timeout=15)
            log.close()

if __name__=='__main__':main()
