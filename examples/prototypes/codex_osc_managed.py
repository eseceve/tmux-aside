"""THROWAWAY: observe real Codex OSC 7501 through isolated tmux panes.

Run: python3 examples/prototypes/codex_osc.py
Requires Codex 0.169.0-compatible CLI, tmux and Python 3; no API credentials.
Uses an isolated managed daemon shared by two TUIs and a local fake provider.
Does not change real sessions or configuration.
Evidence is left in the printed temporary directory; all test processes stop.
"""
import json,os,re,shlex,shutil,subprocess,tempfile,threading,time,sys
from pathlib import Path
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
ROOT=Path(tempfile.mkdtemp(prefix='ao-',dir='/tmp')).resolve()
HOME=ROOT/'home';HOME.mkdir();WORK=ROOT/'work';WORK.mkdir()
CODEX=shutil.which('codex')
assert CODEX, 'Install Codex before running this prototype'
NAME='aside-osc-live-'+str(os.getpid())
ENV={k:v for k,v in os.environ.items() if k in ['PATH','LANG','LC_ALL']}
ENV.update(HOME=str(HOME),CODEX_HOME=str(HOME/'codex'),XDG_STATE_HOME=str(ROOT/'state'),MOCK_KEY='prototype-not-a-credential',TERM='xterm-256color')
Path(ENV['CODEX_HOME']).mkdir()
catalog=subprocess.run([CODEX,'debug','models','--bundled'],env=ENV,capture_output=True,text=True,check=True)
MODEL=next(m for m in json.loads(catalog.stdout)['models'] if m['slug']=='gpt-5.5')
requests=[]
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):
  body=json.dumps({'models':[MODEL]}).encode();self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(body)
 def do_POST(self):
  body=self.rfile.read(int(self.headers.get('Content-Length',0)))
  if self.headers.get('Content-Encoding')=='gzip':
   import gzip;body=gzip.decompress(body)
  if self.headers.get('Content-Encoding')=='zstd':
   proc=subprocess.run(['zstd','-d','-q','--stdout'],input=body,capture_output=True,check=True);body=proc.stdout
  data=json.loads(body);requests.append(data)
  (ROOT/'requests.json').write_text(json.dumps(requests,indent=2))
  def names(tools):
   result=[]
   for tool in tools:
    if tool.get('type')=='namespace':result+=names(tool.get('tools',[]))
    else:result.append(tool.get('name',''))
   return result
  toolnames=names(data.get('tools',[]));print('HTTP request',len(requests),'tools',toolnames,flush=True)
  text=json.dumps(data.get('input',[]))
  use_permission='PROTOTYPE_PERMISSION' in text and 'function_call_output' not in text and 'exec_command' in toolnames
  use_question='PROTOTYPE_QUESTION' in text and 'function_call_output' not in text and 'request_user_input' in toolnames
  if use_question:
   args={'questions':[{'id':'prototype_choice','header':'Prototype','question':'Choose a prototype option','options':[{'label':'A (Recommended)','description':'First option'},{'label':'B','description':'Second option'}]}]}
   item={'type':'function_call','id':'fc_question','call_id':'call_question','name':'request_user_input','arguments':json.dumps(args)}
  elif use_permission:
   name=next((n for n in ['exec_command','shell_command','shell'] if n in toolnames),None)
   if name:
    args={'cmd':'printf prototype-permission','sandbox_permissions':'require_escalated','justification':'Prototype: verify per-pane permission status; do not approve.'}
    if name=='shell_command':args['command']=args.pop('cmd')
    if name=='shell':args['command']=['/bin/sh','-c',args.pop('cmd')]
    item={'type':'function_call','id':'fc_proto','call_id':'call_proto','name':name,'arguments':json.dumps(args)}
   else:item={'type':'message','id':'msg_proto','role':'assistant','content':[{'type':'output_text','text':'Missing shell tool in prototype: '+str(toolnames)}]}
  else:item={'type':'message','id':'msg_proto','role':'assistant','content':[{'type':'output_text','text':'PROTOTYPE_DONE'}]}
  events=[{'type':'response.created','response':{'id':'resp_proto'}},{'type':'response.output_item.done','output_index':0,'item':item},{'type':'response.completed','response':{'id':'resp_proto','status':'completed','output':[item],'usage':{'input_tokens':10,'output_tokens':5,'total_tokens':15}}}]
  self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
  time.sleep(.5)
  for event in events:
   self.wfile.write(('event: '+event['type']+'\ndata: '+json.dumps(event)+'\n\n').encode());self.wfile.flush()
server=ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
config=f'''model = "{MODEL['slug']}"
model_provider = "prototype"
approval_policy = "on-request"
sandbox_mode = "read-only"
web_search = "disabled"
check_for_update_on_startup = false
[features]
plugins = false
code_mode = false
code_mode_host = false
[model_providers.prototype]
name = "Local prototype"
base_url = "http://127.0.0.1:{server.server_port}/v1"
model_catalog_url = "http://127.0.0.1:{server.server_port}/models"
env_key = "MOCK_KEY"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
request_max_retries = 0
stream_max_retries = 0
[projects.{json.dumps(str(WORK))}]
trust_level = "trusted"
'''
(Path(ENV['CODEX_HOME'])/'config.toml').write_text(config)
def tmux(*args):return subprocess.check_output(['tmux','-L',NAME,*args],text=True,env=ENV).strip()
def screen(p):return tmux('capture-pane','-p','-t',p)
def wait(condition,label,timeout=35):
 end=time.monotonic()+timeout
 while time.monotonic()<end:
  if condition():return
  time.sleep(.2)
 raise RuntimeError('Timed out: '+label)
def states(p):
 raw=(ROOT/(p[1:]+'.raw')).read_bytes() if (ROOT/(p[1:]+'.raw')).exists() else b''
 return [m.decode() for m in re.findall(rb'\x1b\]7501;([^\x1b\x07]+)(?:\x1b\\|\x07)',raw)]
def enter(p,text):
 tmux('send-keys','-t',p,'-l',text);time.sleep(.4);tmux('send-keys','-t',p,'Enter')
app=None;plugin=None;panes=[]
try:
 # Both clients share this one real app-server, whose inherited pane is deliberately %0.
 p0=tmux('-f','/dev/null','new-session','-d','-x','140','-y','40','-s','research','-P','-F','#{pane_id}','-c',str(WORK),'sleep 300');panes.append(p0)
 socket_path=tmux('display-message','-p','#{socket_path}')
 ENV.update(TMUX=socket_path+',0,0',TMUX_PANE=p0)
 subprocess.run([CODEX,'app-server','daemon','start'],env=ENV,cwd=WORK,check=True,start_new_session=True)
 version=subprocess.run([CODEX,'app-server','daemon','version'],env=ENV,cwd=WORK,capture_output=True,text=True,check=True)
 (ROOT/'daemon-version.json').write_text(version.stdout)
 print('Managed daemon:',version.stdout,flush=True)
 p1=tmux('new-window','-d','-P','-F','#{pane_id}','-t','research','-c',str(WORK),'sleep 300');panes.append(p1)
 for p in panes:
  # Set pipe after respawn but hold the client behind a gate so no initial OSC is lost.
  command=f'tmux -L {NAME} wait-for ready{p[1:]}; exec {shlex.quote(CODEX)}'
  tmux('respawn-pane','-k','-t',p,command)
  tmux('pipe-pane','-O','-t',p,'cat > '+shlex.quote(str(ROOT/(p[1:]+'.raw'))))
  tmux('wait-for','-S','ready'+p[1:])
 time.sleep(4)
 # Stop early with diagnostic evidence if onboarding needs a separate scripted step.
 for p in panes:
  wait(lambda:bool(states(p)),'first OSC '+p)
  print('OSC initial',p,states(p),flush=True)
 enter(p0,'PROTOTYPE_PERMISSION')
 enter(p1,'PROTOTYPE_FINISH')
 wait(lambda:any('state=blocked' in x for x in states(p0)),'permission OSC',45)
 wait(lambda:any('state=done' in x for x in states(p1)),'done OSC',45)
 assert not any('state=blocked' in x for x in states(p1)),states(p1)
 print('VERIFIED separate pane states',json.dumps({p:states(p) for p in panes}),flush=True)
 tmux('send-keys','-t',p0,'C-c')
 wait(lambda:states(p0)[-1]=='state=idle:app=codex','interruption returns idle')
 print('VERIFIED interruption',states(p0),flush=True)
 enter(p0,'/new')
 time.sleep(1)
 enter(p0,'/plan')
 time.sleep(1)
 enter(p0,'PROTOTYPE_QUESTION')
 wait(lambda:any('kind=question' in x for x in states(p0)),'question OSC',30)
 assert not any('state=blocked' in x for x in states(p1))
 print('VERIFIED question and new conversation',states(p0),flush=True)
 # A newly attached observer receives future output only, not the cached OSC state.
 late=ROOT/'late.raw'
 tmux('pipe-pane','-O','-t',p1,'cat > '+shlex.quote(str(late)))
 time.sleep(1)
 raw=late.read_bytes() if late.exists() else b''
 assert b'\x1b]7501;' not in raw, 'Unexpected cached state re-emission'
 print('VERIFIED late observer begins unknown, not idle',flush=True)
 enter(p1,'PROTOTYPE_FINISH_AGAIN')
 wait(lambda:late.exists() and b'state=done:app=codex' in late.read_bytes(),'late observer sees next done')
 print('VERIFIED late observer receives next transition',flush=True)

 # Exercise the compiled observer against the same real managed clients.
 binary=Path(__file__).resolve().parents[2]/'target/debug/tmux-aside-agents'
 for p in panes:tmux('pipe-pane','-t',p)
 ENV['TMUX_ASIDE_LANG']='en'
 plugin=subprocess.Popen([str(binary)],env=ENV,cwd=WORK,stdin=subprocess.PIPE,stdout=(ROOT/'plugin.jsonl').open('w'),stderr=(ROOT/'plugin.stderr').open('w'))
 plugin.stdin.write(b'{"type":"hello","protocol":1,"features":[]}\n');plugin.stdin.flush()
 def observed(p):
  try:return json.loads((ROOT/'state/tmux-aside/agents'/(p[1:]+'.json')).read_text())
  except (FileNotFoundError,json.JSONDecodeError):return {}
 wait(lambda:all(observed(p).get('terminal',{}).get('state')=='unknown' for p in panes),'late plugin starts unknown')
 tmux('send-keys','-t',p0,'C-c');time.sleep(1)
 enter(p0,'/new');time.sleep(1)
 enter(p0,'/plan');time.sleep(1)
 enter(p0,'PROTOTYPE_QUESTION')
 enter(p1,'PROTOTYPE_FINISH_PLUGIN')
 wait(lambda:observed(p0).get('status')=='waiting' and observed(p1).get('status')=='done','compiled plugin independent statuses')
 assert observed(p0)['terminal']['process_id'] != observed(p1)['terminal']['process_id']
 (ROOT/'plugin-states.json').write_text(json.dumps({p:observed(p) for p in panes},indent=2))
 print('VERIFIED compiled plugin: independent waiting and done with managed daemon',flush=True)

except Exception as e:
 print('ERROR',str(e),flush=True)
 for p in panes:
  try:(ROOT/(p[1:]+'.screen')).write_text(screen(p));print(p,'states',states(p),'screen',screen(p)[-3000:],flush=True)
  except Exception:pass
 raise
finally:
 if plugin:
  plugin.stdin.close()
  try:plugin.wait(timeout=5)
  except subprocess.TimeoutExpired:plugin.kill();plugin.wait()
 subprocess.run(['tmux','-L',NAME,'kill-server'],env=ENV,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 subprocess.run([CODEX,'app-server','daemon','stop'],env=ENV,cwd=WORK,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 if app:
  app.terminate()
  try:app.wait(timeout=5)
  except subprocess.TimeoutExpired:app.kill();app.wait()
 # Codex stop leaves its package-update loop alive; stop only our isolated package.
 processes=subprocess.check_output(['ps','-axo','pid=,command='],text=True)
 for line in processes.splitlines():
  parts=line.strip().split(None,1)
  if len(parts)==2 and parts[1].startswith(str(ROOT)+'/') and 'app-server daemon pid-update-loop' in parts[1]:
   try:os.kill(int(parts[0]),15)
   except ProcessLookupError:pass
 server.shutdown();print('Evidence:',ROOT,flush=True)
