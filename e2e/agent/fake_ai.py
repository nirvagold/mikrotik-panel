import json, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
LOG=open('ai.log','a')
def chunk(d): return 'data: '+json.dumps({'choices':[d]})+'\n\n'
def tool_calls(calls):
    out=[]
    for i,(name,args) in enumerate(calls):
        out.append(chunk({'delta':{'tool_calls':[{'index':i,'id':f'call_{time.time_ns()}_{i}','type':'function','function':{'name':name,'arguments':json.dumps(args)}}]}}))
    out.append(chunk({'delta':{},'finish_reason':'tool_calls'}))
    return out
def text(t):
    return [chunk({'delta':{'content':w+' '}}) for w in t.split(' ')]+[chunk({'delta':{},'finish_reason':'stop'})]
class H(BaseHTTPRequestHandler):
    protocol_version='HTTP/1.1'
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['content-length'])))
        msgs=body['messages']; last=msgs[-1]
        LOG.write(json.dumps({'last':last,'n':len(msgs),'tools':len(body.get('tools',[]))})+'\n'); LOG.flush()
        users=[m for m in msgs if m['role']=='user' and isinstance(m.get('content'),str) and '[Konteks panel]' not in m['content']]
        ut=json.dumps(msgs[-1]) if last['role']=='user' else ''
        if last['role']=='tool':
            # find what we did
            first_user=[m for m in msgs if m['role']=='user'][-1]
            res=last['content']
            parts=text('Selesai. Warna aksen sekarang hijau toska dan ada catatan baru di Ringkasan. Hasil alat: '+res[:80].replace('\n',' '))
        elif 'putus' in ut:
            parts=[chunk({'delta':{'content':'Saya siapkan perintahnya, perlu persetujuan Anda. '}})]+tool_calls([('propose_command',{'command':{'path':['ppp','active'],'verb':'remove','find':{'name':'budi'}},'reason':'Pelanggan minta sesi diputus.'})])
        elif 'lambat' in ut:
            parts=text(' '.join(['kata%d'%i for i in range(200)]))
        else:
            parts=[chunk({'delta':{'content':'Oke, saya ubah warnanya. '}})]+tool_calls([
              ('set_tokens',{'changes':{'light':{'accent':'#0f766e'},'dark':{'accent':'#2dd4bf'},'radius':18}}),
              ('add_block',{'page':'ringkasan','index':0,'block':{'id':'catatan-agen','span':1,'kind':'text','props':{'markdown':'**Catatan dari agen:** desain diubah langsung tanpa reload.'}}}),
            ])
        self.send_response(200); self.send_header('content-type','text/event-stream'); self.send_header('transfer-encoding','chunked'); self.end_headers()
        for p in parts+['data: [DONE]\n\n']:
            b=p.encode(); self.wfile.write(b'%x\r\n'%len(b)+b+b'\r\n'); self.wfile.flush()
            time.sleep(0.08 if 'lambat' not in ut else 0.15)
        self.wfile.write(b'0\r\n\r\n')
    def log_message(self,*a): pass
ThreadingHTTPServer(('127.0.0.1',int(sys.argv[1])),H).serve_forever()
