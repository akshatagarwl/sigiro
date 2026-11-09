import hashlib,json,os,secrets,time,urllib.request,urllib.error
HTTP_PORT = int(os.environ.get('SIGIRO_E2E_HTTP_PORT', '19997'))
OTLP_PORT = int(os.environ.get('SIGIRO_E2E_OTLP_PORT', '14348'))
from pathlib import Path

def post(port,path,payload):
 r=urllib.request.urlopen(urllib.request.Request(f'http://127.0.0.1:{port}{path}',data=json.dumps(payload).encode(),headers={'content-type':'application/json'}));return r.read()

def run(count):
 trace=secrets.token_hex(16);root=secrets.token_hex(8);start=time.time_ns();spans=[]
 for i in range(count):
  sid=secrets.token_hex(8);t=time.time_ns();payload=f'Sigiro local browser verification operation {i}'.encode()
  digest=hashlib.pbkdf2_hmac('sha256',payload,b'local-ui-check',10000).hex();end=time.time_ns()
  spans.append({'traceId':trace,'spanId':sid,'parentSpanId':root,'name':f'pbkdf2 operation {i}','kind':1,'startTimeUnixNano':str(t),'endTimeUnixNano':str(end),'attributes':[{'key':'operation.input','value':{'stringValue':payload.decode()}},{'key':'operation.output','value':{'stringValue':digest}},{'key':'iterations','value':{'intValue':'10000'}}],'status':{'code':1},'events':[{'timeUnixNano':str(end),'name':'hash computed','attributes':[]}]})
 t=time.time_ns()
 try:urllib.request.urlopen(f'http://127.0.0.1:{HTTP_PORT}/does-not-exist')
 except urllib.error.HTTPError as e:
  spans.append({'traceId':trace,'spanId':secrets.token_hex(8),'parentSpanId':root,'name':'GET /does-not-exist','kind':3,'startTimeUnixNano':str(t),'endTimeUnixNano':str(time.time_ns()),'status':{'code':2,'message':str(e)},'attributes':[{'key':'http.response.status_code','value':{'intValue':str(e.code)}}]})
 end=time.time_ns()
 spans.append({'traceId':trace,'spanId':root,'name':f'UI verification workload ({count} hashes)','kind':1,'startTimeUnixNano':str(start),'endTimeUnixNano':str(end),'attributes':[{'key':'verification.workload','value':{'boolValue':True}}],'status':{'code':1}})
 body={'resourceSpans':[{'resource':{'attributes':[{'key':'service.name','value':{'stringValue':'dashboard-e2e'}}]},'scopeSpans':[{'scope':{'name':'local-browser-verification'},'spans':spans}]}]}
 post(OTLP_PORT,'/v1/traces',body)
 post(OTLP_PORT,'/v1/logs',{'resourceLogs':[{'resource':{'attributes':[{'key':'service.name','value':{'stringValue':'dashboard-e2e'}}]},'scopeLogs':[{'logRecords':[{'timeUnixNano':str(end),'severityNumber':9,'severityText':'INFO','body':{'stringValue':f'Completed {count} measured PBKDF2 operations'},'traceId':trace,'spanId':root}]}]}]})
 return {'trace':trace,'root':root,'spans':len(spans),'operations':count}
result={'normal':run(80),'large':run(2100)}
Path(os.environ.get('TRACE_FIXTURE', '/tmp/sigiro-trace-workload.json')).write_text(json.dumps(result,indent=2));print(result)
