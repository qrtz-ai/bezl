# Echo Linux cross-check (docs/echo/dictation-tuning.md §6, ADR 0102). Dev-box only: never part of the app, and no
# network at run time (models and LibriSpeech are local files; see README.md). Feeds each utterance to a sherpa-onnx
# recogniser in 85 ms blocks like the Mac tap, pads, finishes, and scores WER (normalised like qrtz-talk::bench),
# "last word missed" and the release-side decode time. Writes results/<name>.json (summary + per-utterance hyps).
import sherpa_onnx, soundfile as sf, numpy as np, time, sys, json, re, argparse, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__))); import dsp
p=argparse.ArgumentParser(); p.add_argument("model"); p.add_argument("--name"); p.add_argument("--pad",type=float,default=0.64)
p.add_argument("--trim",action="store_true",help="trim trailing silence to last speech (fast release)")
p.add_argument("--offline",action="store_true"); p.add_argument("--threads",type=int,default=4)
p.add_argument("--chunk",type=float,default=0.085,help="feed block seconds (mac tap ~85ms)")
p.add_argument("--chunkpad",type=int,default=0,help="pad the remainder to a multiple of N samples (FluidAudio finish())")
p.add_argument("--trimkeep",type=float,default=0.04,help="seconds kept after the last speech frame with --trim");
p.add_argument("--via48",choices=("linear","sinc"),default=None,help="mic at 48 kHz: up to 48 kHz (dsp.up48), then back to 16 kHz with the helper's linear DSP.Resampler or a windowed sinc (ADR 0104)")
p.add_argument("--noise",type=float,default=None,help="white noise at this SNR dB vs the clip RMS, seeded per utterance (added at 48 kHz with --via48)")
p.add_argument("--band",choices=("hf",),default=None,help="with --via48 --noise: 8-20 kHz noise only (out of the model's band)")
p.add_argument("--scale",type=float,default=None,help="input gain, e.g. 0.1 for a quiet mic"); p.add_argument("--norm",choices=("peak","agc"),default=None,help="after --scale: whole-clip peak (oracle) or causal per-block normaliser (dsp.py), to --target peak with at most --cap gain")
p.add_argument("--target",type=float,default=0.9,help="--norm target peak"); p.add_argument("--cap",type=float,default=10.0,help="--norm gain cap")
p.add_argument("--keep",type=int,default=None,help="ms kept after the last speech frame with --trim (overrides --trimkeep; §7 rows)")
p.add_argument("--hf",type=float,default=None,help="§7 rows: via 48 kHz with 8-20 kHz noise at this SNR dB (dsp.hf_band_via48)"); p.add_argument("--resampler",choices=("linear","poly"),default="poly",help="with --hf: back to 16 kHz linear (DSP.Resampler) or polyphase")
p.add_argument("--headdrop",type=int,default=None,help="cold mic: leading silence cut to 20 ms before speech, then this many ms dropped (dsp.head_drop)")
p.add_argument("--lang",default=None,help="per-stream language option (Nemotron 3.5: en, auto, ...)"); p.add_argument("--hotwords",default=None); p.add_argument("--limit",type=int,default=0)
a=p.parse_args(); d=a.model
from score import norm, ed
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
res=[]; E={}; N={}; tl=0; hlt=0; aud=0; comp=0; finals=[]
for split,i,path,ref in S:
    x,sr=sf.read(path,dtype="float32")
    if a.trim: x=trim(x,sr,a.trimkeep if a.keep is None else a.keep/1000)
    if a.hf is not None: x=dsp.hf_band_via48(x,a.hf,i,a.resampler)
    if a.headdrop is not None: x=dsp.head_drop(x,sr,a.headdrop)
    if a.via48:
        y=dsp.up48(x)
        if a.noise is not None: y=dsp.white_noise(y,a.noise,i,a.band)
        x=dsp.swift_linear(y,48000,16000) if a.via48=="linear" else dsp.sinc(y)
    elif a.noise is not None: x=dsp.white_noise(x,a.noise,i)
    if a.scale is not None: x=(x*a.scale).astype(np.float32)
    if a.norm=="peak": x=dsp.peak_norm(x,a.target,a.cap)
    elif a.norm=="agc": x=dsp.agc(x,a.target,a.cap)
    aud+=len(x)/sr
    if a.offline:
        t=time.time(); s=r.create_stream(); s.accept_waveform(sr,x); r.decode_stream(s); hyp=s.result.text; fin=time.time()-t; comp+=fin
    else:
        s=r.create_stream(); b=int(sr*a.chunk); t=time.time()
        if a.lang: s.set_option("language",a.lang)
        for k in range(0,len(x),b):
            s.accept_waveform(sr,x[k:k+b])
            while r.is_ready(s): r.decode_stream(s)
        comp+=time.time()-t; t=time.time()
        if a.pad>0: s.accept_waveform(sr,np.zeros(int(sr*a.pad),np.float32))
        if a.chunkpad:  # finish(): the remainder after the pad, padded to a chunk (with no pad and no remainder, one whole chunk: §6's finishpad row)
            npad=int(sr*a.pad) if a.pad>0 else 0; rem=(-(len(x)+npad))%a.chunkpad
            s.accept_waveform(sr,np.zeros(rem if (rem or npad) else a.chunkpad,np.float32))
        s.input_finished()
        while r.is_ready(s): r.decode_stream(s)
        fin=time.time()-t; comp+=fin; hyp=r.get_result(s)
    R,H=norm(ref),norm(hyp); e=ed(R,H); E[split]=E.get(split,0)+e; N[split]=N.get(split,0)+len(R)
    # tail loss: reference final word missing from hypothesis's last 2 words
    lost = 1 if R and R[-1] not in H[-2:] else 0; tl+=lost; finals.append(fin)
    hl = 1 if R and R[0] not in H[:2] else 0; hlt+=hl  # head loss: reference first word not among the first 2
    res.append(dict(split=split,id=i,wer=e/max(1,len(R)),tailLost=lost,headLost=hl,finalMs=fin*1000,hyp=hyp))
f=np.array(finals)*1000
summ=dict(name=a.name,pad=a.pad,trim=a.trim,offline=a.offline,via48=a.via48,noise=a.noise,band=a.band,hf=a.hf,resampler=a.resampler if a.hf is not None else None,headdrop=a.headdrop,keep=a.keep,scale=a.scale,norm=a.norm,target=a.target if a.norm else None,cap=a.cap if a.norm else None,lang=a.lang,threads=a.threads,n=len(S),loadS=round(load,1),
  wer={k:round(100*E[k]/N[k],2) for k in E}, werAll=round(100*sum(E.values())/sum(N.values()),2),
  tailLostPct=round(100*tl/len(S),1), headLostPct=round(100*hlt/len(S),1), rtf=round(comp/aud,3), finalMsP50=round(float(np.percentile(f,50)),1), finalMsP90=round(float(np.percentile(f,90)),1))
print(json.dumps(summ))
json.dump(dict(summary=summ,runs=res),open(f"results/{a.name}.json","w"))
