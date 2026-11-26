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
        elif 'batasi-salah' in ut:
            parts=tool_calls([('propose_command',{'command':{'path':['ip','service'],'verb':'set','find':{'name':'api'},'args':{'address':'10.10.10.0/24'}},'reason':'Batasi API ke LAN.'})])
        elif 'batasi' in ut:
            parts=tool_calls([('propose_command',{'command':{'path':['ip','service'],'verb':'set','find':{'name':'api'},'args':{'address':'10.10.10.0/24,127.0.0.1/32'}},'reason':'Batasi API ke LAN dan server panel.'})])
        elif 'kartu' in ut:
            parts=[chunk({'delta':{'content':'Ini datanya. '}})]+tool_calls([
              ('show_card',{'kind':'table','title':'Pelanggan teratas','data':{'columns':['Nama','Pakai','Status'],'rows':[['budi','12,4 GiB','online'],['sari','8,1 GiB','offline'],['andi','3,0 GiB','online']]}}),
              ('show_card',{'kind':'chart','title':'Pemakaian per pelanggan','data':{'type':'bar','unit':'GiB','labels':['budi','sari','andi'],'series':[{'name':'Unduh','values':[12.4,8.1,3.0]},{'name':'Unggah','values':[2.1,1.4,0.6]}]}}),
              ('show_card',{'kind':'chart','title':'Trafik 24 jam','data':{'type':'line','unit':'Mbps','labels':['00:00','06:00','12:00','18:00','23:00'],'series':[{'name':'Unduh','values':[20,8,45,80,35]}]}}),
              ('show_card',{'kind':'topology','title':'Topologi','data':{'router':'MikroTik-Core','interfaces':[{'name':'ether1','type':'ether','running':True},{'name':'bridge-lan','type':'bridge','running':True,'arp_hosts':14},{'name':'ether2','type':'ether','running':True},{'name':'ether3','type':'ether','running':False},{'name':'vlan10','type':'vlan','running':True}],'bridges':{'bridge-lan':[{'interface':'ether2'},{'interface':'ether3'}]},'vlans':[{'name':'vlan10','interface':'bridge-lan'}],'neighbors':[{'interface':'ether1','identity':'ISP-GW','address':'100.64.0.1','mac-address':'AA'}]}}),
            ])
        elif 'putus' in ut:
            parts=[chunk({'delta':{'content':'Saya siapkan perintahnya, perlu persetujuan Anda. '}})]+tool_calls([('propose_command',{'command':{'path':['ppp','active'],'verb':'remove','find':{'name':'budi'}},'reason':'Pelanggan minta sesi diputus.'})])
        elif 'lambat' in ut:
            parts=text(' '.join(['kata%d'%i for i in range(200)]))
        else:
            parts=[chunk({'delta':{'content':'Oke, saya ubah warnanya. '}})]+tool_calls([
              ('set_tokens',{'changes':{'dark':{'accent':'#2dd4bf'},'radius':12}}),
              ('add_block',{'page':'ringkasan','index':0,'block':{'id':'catatan-agen','span':1,'kind':'text','props':{'markdown':'**Catatan dari agen:** desain diubah langsung tanpa reload.'}}}),
            ])
        self.send_response(200); self.send_header('content-type','text/event-stream'); self.send_header('transfer-encoding','chunked'); self.end_headers()
        for p in parts+['data: [DONE]\n\n']:
            b=p.encode(); self.wfile.write(b'%x\r\n'%len(b)+b+b'\r\n'); self.wfile.flush()
            time.sleep(0.08 if 'lambat' not in ut else 0.15)
        self.wfile.write(b'0\r\n\r\n')
    def do_GET(self):
        ok=self.headers.get('authorization')=='Bearer sk-test'
        b=json.dumps({'object':'list','data':[{'id':'gpt-mini'},{'id':'deepseek-chat'},{'id':'qwen-max'}]} if ok else {'error':{'message':'bad key'}}).encode()
        self.send_response(200 if ok else 401); self.send_header('content-type','application/json'); self.send_header('content-length',str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self,*a): pass
ThreadingHTTPServer(('127.0.0.1',int(sys.argv[1])),H).serve_forever()
