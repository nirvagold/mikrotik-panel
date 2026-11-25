import socket, threading, sys, json, datetime
LOG=open('router.log','a')
def enc_len(n):
    if n<0x80: return bytes([n])
    if n<0x4000: return (n|0x8000).to_bytes(2,'big')
    if n<0x200000: return (n|0xC00000).to_bytes(3,'big')
    return (n|0xE0000000).to_bytes(4,'big')
def rd(s,n):
    b=b''
    while len(b)<n:
        c=s.recv(n-len(b))
        if not c: raise EOFError
        b+=c
    return b
def read_len(s):
    c=rd(s,1)[0]
    if c<0x80: return c
    if c<0xC0: return ((c&0x3F)<<8)|rd(s,1)[0]
    if c<0xE0: return ((c&0x1F)<<16)|int.from_bytes(rd(s,2),'big')
    return ((c&0x0F)<<24)|int.from_bytes(rd(s,3),'big')
def read_sentence(s):
    w=[]
    while True:
        n=read_len(s)
        if n==0: return w
        w.append(rd(s,n).decode())
def send(s,words):
    s.sendall(b''.join(enc_len(len(x.encode()))+x.encode() for x in words)+b'\0')
now=datetime.datetime.utcnow()+datetime.timedelta(hours=6)
users=['budi','sari','andi']
T={
 '/system/resource/print':[{'cpu-load':'7','version':'7.16.2','free-memory':'180000000','total-memory':'268435456','uptime':'3d4h','board-name':'RB4011'}],
 '/system/identity/print':[{'name':'core-rt'}],
 '/user-manager/user/print':[{'.id':f'*{i}','name':u,'disabled':'false'} for i,u in enumerate(users,1)],
 '/user-manager/user-profile/print':[{'.id':f'*1{i}','user':u,'profile':f'p-{u}','state':'running-active'} for i,u in enumerate(users)],
 '/user-manager/profile-limitation/print':[{'.id':f'*2{i}','profile':f'p-{u}','limitation':f'l-{u}'} for i,u in enumerate(users)],
 '/user-manager/limitation/print':[{'.id':f'*3{i}','name':f'l-{u}','transfer-limit':str(10737418240*(i+1)),'reset-counters-start-time':'2026-11-01 00:00:00','reset-counters-interval':'monthly'} for i,u in enumerate(users)],
 '/ppp/active/print':[{'.id':'*a1','name':'budi','service':'pppoe','address':'10.0.0.2','uptime':'1h'},{'.id':'*a2','name':'sari','service':'pppoe','address':'10.0.0.3','uptime':'2h'}],
 '/interface/print':[{'name':'ether1','type':'ether','rx-byte':'5000000','tx-byte':'9000000','running':'true'}],
 '/user-manager/session/print':[{'acct-session-id':'a1','user':'budi','started':(now-datetime.timedelta(hours=1)).strftime('%Y-%m-%d %H:%M:%S'),'download':'500000000','upload':'50000000'}],
}
def handle(s):
    try:
        while True:
            w=read_sentence(s)
            if not w: continue
            path=w[0]; LOG.write(json.dumps(w)+'\n'); LOG.flush()
            filters=[x[1:].split('=',1) for x in w[1:] if x.startswith('?') and '=' in x]
            rows=[r for r in T.get(path,[]) if all(r.get(k)==v for k,v in filters)]
            for r in rows: send(s,['!re']+[f'={k}={v}' for k,v in r.items()])
            tag=[x for x in w[1:] if x.startswith('.tag=')]
            send(s,['!done']+tag)
    except EOFError: pass
srv=socket.socket(); srv.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); srv.bind(('127.0.0.1',int(sys.argv[1]))); srv.listen()
while True:
    c,_=srv.accept(); threading.Thread(target=handle,args=(c,),daemon=True).start()
