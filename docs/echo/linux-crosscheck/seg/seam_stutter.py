"""Echo cycle 76 (ADR 0227): what segmented Parakeet TDT v2 writes when a speaker repeats a word after a pause long
enough to cut a segment ("talk to the ... the design team"). Splices real LibriSpeech audio: the clip up to the end of
a word W, a pause, then the clip again from the start of W. The helper's PauseSegmenter cuts in the middle of a pause of
300 ms or more once a segment has 2 s, and decodes each segment on its own with 640 ms of zero pad (ADR 0118); the
stitcher appends each segment's words. So seg1 = clip[:end(W)] + half the pause, seg2 = half the pause + clip[start(W):],
each decoded with the pad, joined with one space. Dev-box only, no network (local model + LibriSpeech).
usage: venv/bin/python seam_stutter.py [--pause 0.6] [--limit N] > seam.jsonl"""
import sherpa_onnx, soundfile as sf, numpy as np, json, sys, argparse, os, re
p = argparse.ArgumentParser(); p.add_argument("--pause", type=float, default=0.6); p.add_argument("--limit", type=int, default=300)
p.add_argument("--words", default="the,a,an,i,to,and,we,you,it"); p.add_argument("--per", type=int, default=1)
a = p.parse_args()
d = "models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8"
r = sherpa_onnx.OfflineRecognizer.from_transducer(model_type="nemo_transducer", feature_dim=128, num_threads=4,
    tokens=f"{d}/tokens.txt", encoder=f"{d}/encoder.int8.onnx", decoder=f"{d}/decoder.int8.onnx", joiner=f"{d}/joiner.int8.onnx")
PAD = np.zeros(int(16000 * 0.64), np.float32)
def dec(x):
    s = r.create_stream(); s.accept_waveform(16000, np.concatenate([x, PAD])); r.decode_stream(s); return s.result
want = set(a.words.split(","))
S = json.load(open("evalset.json"))[: a.limit]
out = []
for split, uid, path, ref in S:
    x, sr = sf.read(path, dtype="float32"); assert sr == 16000
    res = dec(x); toks, ts = list(res.tokens), list(res.timestamps)
    # words from tokens: sherpa writes a word-opening token with a leading space
    words = []
    for k, t in enumerate(toks):
        if t.startswith(" ") or not words: words.append([t.strip(), k, k])
        else: words[-1][0] += t; words[-1][2] = k
    used = {}
    for wi, (w, k0, k1) in enumerate(words[:-1]):
        bare = re.sub(r"[^\w']", "", w).lower()
        if bare not in want or w != re.sub(r"[^\w']", "", w) or used.get(bare, 0) >= a.per: continue
        start, nxt = ts[k0], ts[words[wi + 1][1]]
        # the cut: the quietest 10 ms between the word's first frame + 60 ms and the next word's first frame + 40 ms
        lo, hi = int((start + 0.06) * 16000), int((nxt + 0.04) * 16000)
        fr = [(float(np.sqrt(np.mean(x[i:i + 160] ** 2))), i) for i in range(lo, max(lo + 160, hi - 160), 160)]
        end = min(fr)[1] / 16000 if fr else nxt; start = max(0.0, start - 0.04)
        if start < 2.0 or len(x) / 16000 - end < 0.8: continue
        used[bare] = used.get(bare, 0) + 1
        s0, e0 = int(start * 16000), int(end * 16000); half = np.zeros(int(16000 * a.pause / 2), np.float32)
        t1 = dec(np.concatenate([x[:e0], half])).text.strip(); t2 = dec(np.concatenate([half, x[s0:]])).text.strip()
        print(json.dumps({"id": uid, "word": bare, "whole": res.text.strip(), "seg1": t1, "seg2": t2, "stitched": f"{t1} {t2}"}), flush=True)
        print(bare, "|", t1[-30:], "||", t2[:30], file=sys.stderr, flush=True)
