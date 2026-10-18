# Echo Linux cross-check (docs/echo/dictation-tuning.md §6, ADR 0102). Dev-box only: never part of the app, and no
# network at run time (models and LibriSpeech are local files; see README.md). Feeds each utterance to a sherpa-onnx
# recogniser in 85 ms blocks like the Mac tap, pads, finishes, and scores WER (normalised like qrtz-talk::bench),
# "last word missed" and the release-side decode time. Writes results/<name>.json (summary + per-utterance hyps).
import sherpa_onnx, soundfile as sf, numpy as np, time, sys, json, re, argparse
p=argparse.ArgumentParser(); p.add_argument("model"); p.add_argument("--name"); p.add_argument("--pad",type=float,default=0.64)
p.add_argument("--trim",action="store_true",help="trim trailing silence to last speech (fast release)")
p.add_argument("--offline",action="store_true"); p.add_argument("--threads",type=int,default=4)
p.add_argument("--chunk",type=float,default=0.085,help="feed block seconds (mac tap ~85ms)")
p.add_argument("--chunkpad",type=int,default=0,help="pad the remainder to a multiple of N samples (FluidAudio finish())")
p.add_argument("--trimkeep",type=float,default=0.04,help="seconds kept after the last speech frame with --trim"); p.add_argument("--hotwords",default=None); p.add_argument("--limit",type=int,default=0)
a=p.parse_args(); d=a.model
NUM={w:i for i,w in enumerate("zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen".split())}
for i,w in enumerate("twenty thirty forty fifty sixty seventy eighty ninety".split()): NUM[w]=20+10*i
def norm(s):
    s=s.lower().replace("%"," percent").replace("-"," ")
    w=re.findall(r"[a-z0-9']+",s); o=[]
    for x in w:
        x=x.strip("'")
        if not x: continue
        if x in NUM and o and o[-1].isdigit() and int(o[-1])%10==0 and int(o[-1])>=20 and NUM[x]<10: o[-1]=str(int(o[-1])+NUM[x]); continue
        o.append(str(NUM[x]) if x in NUM else x)
    return o
def ed(r,h):
    D=list(range(len(h)+1))
    for i in range(1,len(r)+1):
        prev=D[0]; D[0]=i
        for j in range(1,len(h)+1):
            cur=D[j]; D[j]=min(D[j]+1,D[j-1]+1,prev+(r[i-1]!=h[j-1])); prev=cur
    return D[-1]
def trim(x,sr,keep=0.04):
    f=int(sr*0.02); e=[np.sqrt(np.mean(x[i:i+f]**2)) for i in range(0,len(x)-f,f)]
    th=max(e)*0.05; last=max([i for i,v in enumerate(e) if v>th]+[0])
    return x[:min(len(x),(last+1)*f+int(sr*keep))]  # keep ~40ms after last speech frame
kw=dict(tokens=f"{d}/tokens.txt",encoder=f"{d}/encoder.int8.onnx",decoder=f"{d}/decoder.int8.onnx",joiner=f"{d}/joiner.int8.onnx",num_threads=a.threads)
t0=time.time()
if a.offline: r=sherpa_onnx.OfflineRecognizer.from_transducer(model_type="nemo_transducer",feature_dim=128,**kw)
else:
    if a.hotwords: kw.update(decoding_method="modified_beam_search",hotwords_file=a.hotwords,hotwords_score=2.0,modeling_unit="bpe",bpe_vocab=f"{d}/bpe.vocab")
    r=sherpa_onnx.OnlineRecognizer.from_transducer(feature_dim=128,**kw)
load=time.time()-t0
S=json.load(open("evalset.json"))
if a.limit: S=S[:a.limit]
res=[]; E={}; N={}; tl=0; aud=0; comp=0; finals=[]
for split,i,path,ref in S:
    x,sr=sf.read(path,dtype="float32")
    if a.trim: x=trim(x,sr,a.trimkeep)
    aud+=len(x)/sr
    if a.offline:
        t=time.time(); s=r.create_stream(); s.accept_waveform(sr,x); r.decode_stream(s); hyp=s.result.text; fin=time.time()-t; comp+=fin
    else:
        s=r.create_stream(); b=int(sr*a.chunk); t=time.time()
        for k in range(0,len(x),b):
            s.accept_waveform(sr,x[k:k+b])
            while r.is_ready(s): r.decode_stream(s)
        comp+=time.time()-t; t=time.time()
        if a.pad>0: s.accept_waveform(sr,np.zeros(int(sr*a.pad),np.float32))
        if a.chunkpad: s.accept_waveform(sr,np.zeros((-len(x))%a.chunkpad or a.chunkpad,np.float32))
        s.input_finished()
        while r.is_ready(s): r.decode_stream(s)
        fin=time.time()-t; comp+=fin; hyp=r.get_result(s)
    R,H=norm(ref),norm(hyp); e=ed(R,H); E[split]=E.get(split,0)+e; N[split]=N.get(split,0)+len(R)
    # tail loss: reference final word missing from hypothesis's last 2 words
    lost = 1 if R and R[-1] not in H[-2:] else 0; tl+=lost; finals.append(fin)
    res.append(dict(split=split,id=i,wer=e/max(1,len(R)),tailLost=lost,finalMs=fin*1000,hyp=hyp))
f=np.array(finals)*1000
summ=dict(name=a.name,pad=a.pad,trim=a.trim,offline=a.offline,n=len(S),loadS=round(load,1),
  wer={k:round(100*E[k]/N[k],2) for k in E}, werAll=round(100*sum(E.values())/sum(N.values()),2),
  tailLostPct=round(100*tl/len(S),1), rtf=round(comp/aud,3), finalMsP50=round(float(np.percentile(f,50)),1), finalMsP90=round(float(np.percentile(f,90)),1))
print(json.dumps(summ))
json.dump(dict(summary=summ,runs=res),open(f"results/{a.name}.json","w"))
