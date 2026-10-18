"""Echo/Parakeet segmenting eval (ADR 0118): long dictations from LibriSpeech, whole-clip vs pause segmentation.

build: python seg_eval.py build  -> seg/dictations.json + seg/wav/*.wav
run:   python seg_eval.py run --name N [--whole] [--silence MS] [--cap S] [--overlap S] [--ovmode all|cap] [--minseg S]
Decodes are cached per exact sample range (seg/cache.json), so configs sharing segments decode once.
"""
import argparse, glob, json, os, random, re, sys, time
import numpy as np, soundfile as sf

ROOT = os.path.dirname(os.path.abspath(__file__)); BASE = os.path.dirname(ROOT)
SR = 16000; FRAME = 320  # 20 ms
NUM = {w: i for i, w in enumerate("zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen".split())}
for i, w in enumerate("twenty thirty forty fifty sixty seventy eighty ninety".split()): NUM[w] = 20 + 10 * i

def norm_words(words):
    """[(text, t)] -> normalized [(word, t)] (same rules as run.py / EchoWER)."""
    out = []
    for w, t in words:
        w = w.lower().replace("%", " percent").replace("-", " ")
        for x in re.findall(r"[a-z0-9']+", w):
            x = x.strip("'")
            if not x: continue
            if x in NUM and out and out[-1][0].isdigit() and int(out[-1][0]) % 10 == 0 and int(out[-1][0]) >= 20 and NUM[x] < 10:
                out[-1] = (str(int(out[-1][0]) + NUM[x]), out[-1][1]); continue
            out.append((str(NUM[x]) if x in NUM else x, t))
    return out

def frame_rms(x):
    n = len(x) // FRAME
    return np.sqrt(np.mean(x[: n * FRAME].reshape(n, FRAME) ** 2, axis=1) + 1e-12)

def speech_bounds(x, rel=0.05):
    e = frame_rms(x); th = max(e.max() * rel, 1e-4); idx = np.where(e > th)[0]
    return (idx[0] * FRAME, (idx[-1] + 1) * FRAME) if len(idx) else (0, len(x))

def build(args):
    rnd = random.Random(11); dicts = []
    os.makedirs(f"{ROOT}/wav", exist_ok=True)
    for split, n in (("test-clean", args.n // 2), ("test-other", args.n - args.n // 2)):
        chapters = sorted(glob.glob(f"{BASE}/data/LibriSpeech/{split}/*/*"))
        rnd.shuffle(chapters); made = 0
        for ch in chapters:
            if made >= n: break
            tr = glob.glob(f"{ch}/*.trans.txt")[0]
            utts = [l.strip().split(" ", 1) for l in open(tr)]
            target = rnd.uniform(20, 60); audio = []; refs = []; parts = []; t = 0.0
            for uid, ref in utts:
                x, _ = sf.read(f"{ch}/{uid}.flac", dtype="float32")
                a, b = speech_bounds(x); a = max(0, a - 960); b = min(len(x), b + 960)  # keep 60 ms margins
                if audio:
                    gap = rnd.uniform(0.25, 1.1); audio.append(np.zeros(int(gap * SR), np.float32)); t += gap
                seg = x[a:b]; parts.append([t, t + len(seg) / SR]); audio.append(seg); refs.append(ref); t += len(seg) / SR
                if t >= target: break
            if t < 20: continue
            y = np.concatenate([np.zeros(int(0.3 * SR), np.float32)] + audio + [np.zeros(int(0.05 * SR), np.float32)])  # quick release
            name = f"{split}-{os.path.basename(os.path.dirname(ch))}-{os.path.basename(ch)}"
            sf.write(f"{ROOT}/wav/{name}.wav", y, SR)
            dicts.append(dict(name=name, split=split, ref=" ".join(refs), seconds=len(y) / SR, utterances=len(refs)))
            made += 1
    json.dump(dicts, open(f"{ROOT}/dictations.json", "w"), indent=1)
    s = [d["seconds"] for d in dicts]
    print(len(dicts), "dictations", round(sum(s) / 60, 1), "min, mean", round(np.mean(s), 1), "s")

class Silence:
    """Streaming silence detector: 20 ms frames; silent below max(2.5 x noise floor, 0.06 x speech level, 0.001)."""
    def __init__(self): self.nf = None; self.sp = 0.02
    def silent(self, r):
        if self.nf is None: self.nf = r
        self.nf = 0.9 * self.nf + 0.1 * r if r < self.nf else self.nf * 1.002
        th = max(2.5 * self.nf, 0.06 * self.sp, 0.001)
        if r >= th: self.sp = 0.95 * self.sp + 0.05 * r
        return r < th

def segment(x, silence_ms, cap_s, minseg_s, pause2_ms=0, after_s=0.0):
    """-> list of (start, end, kind) sample cuts; kind = 'pause' | 'cap' | 'end'. Streaming: only past frames used."""
    e = frame_rms(x); det = Silence(); cuts = []; s0 = 0; run = 0
    need = silence_ms // 20; capf = int(cap_s * SR) // FRAME; minf = int(minseg_s * SR) // FRAME; f0 = 0
    for i, r in enumerate(e):
        sil = det.silent(r); run = run + 1 if sil else 0
        need = (pause2_ms // 20) if pause2_ms and (i + 1 - f0) * FRAME >= after_s * SR else silence_ms // 20
        if run >= need and i + 1 - f0 >= minf:
            cut = (i + 1 - need // 2) * FRAME           # middle of the silence run found so far
            cuts.append((s0, cut, "pause")); s0 = cut; f0 = cut // FRAME; run = 0
        elif i + 1 - f0 >= capf:
            w = e[max(f0, i + 1 - 100): i + 1]           # quietest frame in the last 2 s
            j = max(f0, i + 1 - 100) + int(np.argmin(w)); cut = (j + 1) * FRAME
            if cut <= s0 + FRAME: cut = (i + 1) * FRAME
            cuts.append((s0, cut, "cap")); s0 = cut; f0 = cut // FRAME; run = 0
    cuts.append((s0, len(x), "end"))
    return [c for c in cuts if c[1] > c[0]]

class Decoder:
    def __init__(self, threads):
        import sherpa_onnx
        d = f"{BASE}/models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8"
        self.r = sherpa_onnx.OfflineRecognizer.from_transducer(encoder=f"{d}/encoder.int8.onnx", decoder=f"{d}/decoder.int8.onnx", joiner=f"{d}/joiner.int8.onnx", tokens=f"{d}/tokens.txt", num_threads=threads, model_type="nemo_transducer", feature_dim=128)
        self.cache_path = f"{ROOT}/cache.json"; self.cache = json.load(open(self.cache_path)) if os.path.exists(self.cache_path) else {}
    def words(self, name, x, a, b, pad_s):
        k = f"{name}:{a}:{b}:{pad_s}"
        if k not in self.cache:
            y = np.concatenate([x[a:b], np.zeros(int(pad_s * SR), np.float32)])
            t = time.perf_counter(); s = self.r.create_stream(); s.accept_waveform(SR, y); self.r.decode_stream(s); ms = (time.perf_counter() - t) * 1000
            res = s.result; words = []
            for tok, ts in zip(res.tokens, res.timestamps):
                if tok.startswith(" ") or not words: words.append([tok.strip(), a / SR + ts])
                else: words[-1][0] += tok
            self.cache[k] = dict(words=words, ms=ms, text=res.text)
        return self.cache[k]
    def save(self): json.dump(self.cache, open(self.cache_path, "w"))

def align_errors(R, H):
    """Levenshtein backtrace -> list of hyp-index anchors (float) for each error."""
    n, m = len(R), len(H); D = np.zeros((n + 1, m + 1), np.int32); D[:, 0] = range(n + 1); D[0, :] = range(m + 1)
    for i in range(1, n + 1):
        for j in range(1, m + 1):
            D[i, j] = min(D[i - 1, j] + 1, D[i, j - 1] + 1, D[i - 1, j - 1] + (R[i - 1] != H[j - 1]))
    i, j, errs = n, m, []
    while i > 0 or j > 0:
        if i > 0 and j > 0 and D[i, j] == D[i - 1, j - 1] + (R[i - 1] != H[j - 1]):
            if R[i - 1] != H[j - 1]: errs.append(j - 1)
            i, j = i - 1, j - 1
        elif j > 0 and D[i, j] == D[i, j - 1] + 1: errs.append(j - 1); j -= 1
        else: errs.append(max(0, j - 1)); i -= 1
    return int(D[n, m]), errs

def kind_speech(x, a, b):
    """True when the segment has >= 0.3 s of frames above 0.01 RMS (a blank decode there is a failure)."""
    return int((frame_rms(x[a:b]) > 0.01).sum()) * FRAME >= 0.3 * SR

def run(args):
    dicts = json.load(open(f"{ROOT}/dictations.json")); dec = Decoder(args.threads)
    E = N = 0; per = []; tail_s = []; tail_ms = []; queued_ms = []; nseg = []; ncap = []; lastmiss = 0; berr = 0; bcuts = 0; bwin = []; empties = 0; retries = 0
    for d in dicts:
        x, _ = sf.read(f"{ROOT}/wav/{d['name']}.wav", dtype="float32")
        if args.whole: segs = [(0, len(x), "end")]
        else: segs = segment(x, args.silence, args.cap, args.minseg, args.pause2, args.after)
        words = []; timeline_free = 0.0; queued = 0.0
        for k, (a, b, kind) in enumerate(segs):
            ov = args.overlap if k > 0 and (args.ovmode == "all" or segs[k - 1][2] == "cap") else 0.0
            a2 = max(0, a - int(ov * SR))
            out = dec.words(d["name"], x, a2, b, args.pad)
            if not out["words"] and kind_speech(x, a, b): empties += 1
            if not out["words"] and args.retry and a2 != a and kind_speech(x, a, b):  # TDT can blank a clip that starts mid-word
                out = dec.words(d["name"], x, a, b, args.pad); a2 = a; ov = 0.0; retries += 1
            cut_prev = a / SR; lo = cut_prev - ov / 2 if ov else -1
            mine = [w for w in out["words"] if w[1] >= lo] if k > 0 else out["words"]
            if k > 0 and ov:
                words = [w for w in words if w[1] < lo]
                if args.stitch == "dedupe":  # drop B's leading words that repeat A's last ones (TDT stamps a cut word late)
                    nw = lambda ws: [t for t, _ in norm_words([(w, 0) for w, _ in ws])]
                    for j in range(min(4, len(mine), len(words)), 0, -1):
                        if nw(words[-j:]) == nw(mine[:j]): mine = mine[j:]; break
            words += mine
            # background decoder timeline (serial): a segment is ready at its cut (real time), release at len(x)
            if kind != "end":
                start = max(b / SR, timeline_free); timeline_free = start + out["ms"] / 1000
            else:
                queued = max(0.0, timeline_free - len(x) / SR) * 1000
                tail_s.append((b - a2) / SR + args.pad); tail_ms.append(out["ms"]); queued_ms.append(queued)
        nseg.append(len(segs)); ncap.append(sum(1 for s in segs if s[2] == "cap"))
        R = [w for w, _ in norm_words([(w, 0) for w in d["ref"].split()])]
        HW = norm_words([(w, t) for w, t in words]); H = [w for w, _ in HW]
        e, errs = align_errors(R, H); E += e; N += len(R); per.append(dict(name=d["name"], wer=e / len(R), segments=len(segs)))
        if R and R[-1] not in H[-2:]: lastmiss += 1
        # boundary errors: errors whose hyp time is within 0.5 s of a cut (cuts of the config under test, or --cuts-from)
        cuts = [s[1] / SR for s in (segment(x, args.bsilence, args.bcap, args.minseg) if args.whole and args.bsilence else segs)[:-1]]
        times = [HW[j][1] if j < len(HW) else (HW[-1][1] if HW else 0) for j in errs]
        bcuts += len(cuts); berr += sum(1 for t in times if any(abs(t - c) <= 0.5 for c in cuts))
    if not args.nosave: dec.save()
    q = lambda v, p: round(float(np.percentile(v, p)), 1)
    summ = dict(name=args.name, whole=args.whole, silence=args.silence, cap=args.cap, overlap=args.overlap, ovmode=args.ovmode, stitch=args.stitch, minseg=args.minseg, pause2=args.pause2, after=args.after, pad=args.pad,
                n=len(dicts), wer=round(100 * E / N, 2), errors=E, words=N, lastWordMissedPct=round(100 * lastmiss / len(dicts), 1),
                segmentsMean=round(float(np.mean(nseg)), 1), capCutsMean=round(float(np.mean(ncap)), 2),
                boundaryErrors=berr, cuts=bcuts, boundaryErrorsPerCut=round(berr / max(1, bcuts), 3), emptySegments=empties, retries=retries,
                releaseAudioS=dict(p50=q(tail_s, 50), p90=q(tail_s, 90), max=q(tail_s, 100)),
                releaseCpuMs=dict(p50=q(tail_ms, 50), p90=q(tail_ms, 90), max=q(tail_ms, 100)),
                queuedAtReleaseMs=dict(p50=q(queued_ms, 50), max=q(queued_ms, 100)))
    print(json.dumps(summ)); os.makedirs(f"{ROOT}/results", exist_ok=True)
    json.dump(dict(summary=summ, runs=per), open(f"{ROOT}/results/{args.name}.json", "w"), indent=1)

if __name__ == "__main__":
    p = argparse.ArgumentParser(); p.add_argument("cmd"); p.add_argument("--n", type=int, default=40)
    p.add_argument("--name"); p.add_argument("--whole", action="store_true"); p.add_argument("--silence", type=int, default=300)
    p.add_argument("--cap", type=float, default=15); p.add_argument("--overlap", type=float, default=0.5); p.add_argument("--ovmode", default="all"); p.add_argument("--stitch", default="mid", help="mid | dedupe"); p.add_argument("--nosave", action="store_true"); p.add_argument("--retry", action="store_true"); p.add_argument("--pause2", type=int, default=0, help="shorter pause once the segment is --after s long"); p.add_argument("--after", type=float, default=0.0)
    p.add_argument("--minseg", type=float, default=2.0); p.add_argument("--pad", type=float, default=0.64); p.add_argument("--threads", type=int, default=6)
    p.add_argument("--bsilence", type=int, default=0, help="with --whole: count boundary errors at this config's cuts"); p.add_argument("--bcap", type=float, default=15)
    a = p.parse_args(); build(a) if a.cmd == "build" else run(a)
