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
    if 'hold group turn' in text: cancel.wait(2)
    summary={'bot':os.environ.get('VECTRA_BOT_ID'),'cwd':os.getcwd(),'rules':rules,'mainCredential':bool(os.environ.get('VECTRA_BOT_CAPABILITY')),'prompt':text,'chat':os.environ.get('VECTRA_BOT_CHAT_ID')}
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
            assert b['bot']=='beta' and b['cwd']==str(workspace/'beta') and b['mainCredential'] and b['chat']==beta
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
            before_main=len(messages(main_chat))
            send('main',main_chat,'wait:1.5');wait_for(lambda:main_chat in request('/v1/status')['activeBotChats'])
            delegated={'jobId':'job-callback-1','botId':'beta','chatId':'bot-beta','text':'private delegated work','senderBotId':'main','senderSessionId':main_chat}
            request('/v1/app/bot-jobs',delegated)
            returned=wait_for(lambda:messages(main_chat) if len(messages(main_chat))>=before_main+2 else None)
            assert 'A delegated task has finished' in json.loads(returned[-1]['content'])['prompt']
            wait_for(lambda:not request('/v1/status')['activeBotChats'])
            shared={'requestId':'group-smoke-1','text':'hello shared group','mentions':[]}
            assert request('/v1/app/group',shared)['recipients']==['main']
            def group_replies():return [row for row in request('/v1/app/group')['messages'] if row.get('senderBotId')]
            wait_for(group_replies)
            first=group_replies()[-1];assert first['senderBotId']=='main'
            assert 'private delegated work' not in first['text']
            request('/v1/app/group',shared);assert len([row for row in request('/v1/app/group')['messages'] if row['id']=='group-smoke-1'])==1
            tagged={'requestId':'group-smoke-2','text':'@alpha check this','mentions':['alpha']}
            assert request('/v1/app/group',tagged)['recipients']==['alpha']
            wait_for(lambda:any(row.get('senderBotId')=='alpha' for row in group_replies()))
            assert all(row.get('senderBotId')!='beta' for row in group_replies())
            wait_for(lambda:not request('/v1/status')['activeBotChats'])
            request('/v1/app/group',{'requestId':'group-hold','text':'hold group turn','mentions':['alpha']})
            wait_for(lambda:'group-alpha' in request('/v1/status')['activeBotChats'])
            mention={'requestId':'agent-mention','text':'@beta check the shared topic','mentions':['beta'],'senderBotId':'alpha','senderSessionId':'group-alpha'}
            assert request('/v1/app/group',mention)['recipients']==['beta']
            wait_for(lambda:any(row.get('senderBotId')=='beta' for row in group_replies()))
            wait_for(lambda:not request('/v1/status')['activeBotChats'])
            send('main',main_chat,'wait:1.5');wait_for(lambda:main_chat in request('/v1/status')['activeBotChats'])
            try:
                request('/v1/app/group',{'requestId':'forbidden-dm-share','text':'private text','mentions':['beta'],'senderBotId':'main','senderSessionId':main_chat})
                raise AssertionError('private DM published to group')
            except urllib.error.HTTPError as error:assert error.code==403
            wait_for(lambda:not request('/v1/status')['activeBotChats'])
            import hmac,hashlib,base64
            token=base64.urlsafe_b64encode(hmac.new(('s'*48).encode(),f'vectra-conversation-v1\nalpha\n{alpha}'.encode(),hashlib.sha256).digest()).decode().rstrip('=')
            assert request('/v1/app/bots/authenticate',{'botId':'alpha','sessionId':alpha,'credential':token})['ok']
            try:
                request('/v1/app/bots/authenticate',{'botId':'beta','sessionId':beta,'credential':token})
                raise AssertionError('cross-agent credential accepted')
            except urllib.error.HTTPError as error:assert error.code==401
            before_main=len(messages(main_chat))-2
            send('alpha',alpha,'wait:20');wait_for(lambda:alpha in request('/v1/status')['activeBotChats'])
            process.kill();process.wait(timeout=10);process=start()
            assert messages(alpha,'error'), 'interruption missing from history'
            assert len(messages('bot-beta'))==2
            assert request('/v1/conversations?botId=beta')['conversations']
            assert len(request('/v1/app/group')['messages'])>=4
            assert len(messages(main_chat))==before_main+2
            print('PASS: concurrent bots, scoped credentials, targeting, cancellation, group mentions/default Main, shared-only context, automatic result delivery, idempotency and restart recovery')
        except Exception:
            log.flush();print((root/'daemon.log').read_text()[-7000:]);raise
        finally:
            if process and process.poll() is None:process.terminate();process.wait(timeout=15)
            log.close()

if __name__=='__main__':main()
