"""Cycle 42 (ADR 0129): token confidence of SHORT real release tails, the risk of a confidence rule (confguard.py's
real tails are mostly long). Two sets, each after an utterance and a 500 ms pause, at x1 and x0.1 (gated peak):
  word  : tailguard.py's one-word tails (the first word of another utterance, cut at Parakeet's next-word stamp)
  short : a whole short LibriSpeech utterance (<= 4 words, 0.5-2.5 s) as the tail ("Yes, sir." kind of replies)
usage (from /workspace/echo-linux): venv/bin/python seg/confwords.py -> results/c42-confwords.json"""
import glob, json, os, random, sys
import numpy as np, soundfile as sf
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import nonspeech as N
from confguard import decoder, decode
from helper_seg import segment_helper, Rule as HRule
from seg_eval import norm_words, align_errors, BASE, SR


def main():
    r = decoder(); rnd = random.Random(42)
    files = sorted(glob.glob(f"{BASE}/data/LibriSpeech/test-clean/*/*/*.flac")); rnd.shuffle(files)
    utts = []
    for f in files[600:]:
        x, _ = sf.read(f, dtype="float32")
        if 3 * SR <= len(x) <= 8 * SR: utts.append((os.path.basename(f)[:-5], x))
        if len(utts) >= 60: break
    refs = {}
    for ch in glob.glob(f"{BASE}/data/LibriSpeech/*/*/*/*.trans.txt"):
        for l in open(ch): k, t = l.strip().split(" ", 1); refs[k] = t
    tails = []
    for f in files[800:]:  # one word: same donors as tailguard.py
        if len([t for t in tails if t[0] == "word"]) >= 60: break
        x, _ = sf.read(f, dtype="float32")
        if len(x) > 12 * SR: continue
        y = np.concatenate([x, np.zeros(int(0.64 * SR), np.float32)]); s = r.create_stream(); s.accept_waveform(SR, y); r.decode_stream(s)
        w = []
        for tok, ts in zip(s.result.tokens, s.result.timestamps):
            if tok.startswith(" ") or not w: w.append([tok.strip(), ts])
            else: w[-1][0] += tok
        if len(w) < 3: continue
        a0 = max(0, int((w[0][1] - 0.12) * SR)); a1 = int((w[1][1] - 0.04) * SR); ref = norm_words([(w[0][0], 0)])
        if not ref or a1 - a0 < int(0.12 * SR): continue
        tails.append(("word", ref[0][0], x[a0:a1]))
    short = [f for f in glob.glob(f"{BASE}/data/LibriSpeech/test-*/*/*/*.flac") if len(refs[os.path.basename(f)[:-5]].split()) <= 4]
    random.Random(7).shuffle(short)
    for f in short:
        x, _ = sf.read(f, dtype="float32")
        if 0.5 * SR <= len(x) <= 2.5 * SR: tails.append(("short", refs[os.path.basename(f)[:-5]], x))
        if len([t for t in tails if t[0] == "short"]) >= 60: break
    out = []
    for mic, scale in (("normal", 1.0), ("quiet", 0.1)):
        for j, (src, ref, wav) in enumerate(tails):
            uid, u = utts[j % len(utts)]
            x = np.concatenate([np.zeros(int(0.3 * SR)), u * scale, np.zeros(int(0.5 * SR)), wav * scale, np.zeros(int(0.05 * SR))]).astype(np.float32)
            hs = segment_helper(x, HRule()); a, b, _, has = hs[-1]
            if not has or a < int(0.3 * SR) + len(u): continue  # the tail must be the short speech alone
            y = x[a:b]; y = y * N.peak_gain(y); text, mean, mn = decode(r, y)
            R = [w for w, _ in norm_words([(w, 0) for w in ref.split()])]; H = [w for w, _ in norm_words([(text, 0)])]
            e, _ = align_errors(R, H)
            out.append(dict(src=src, mic=mic, ref=ref, text=text, mean=mean, min=mn, refWords=len(R), errors=e))
    json.dump(out, open(f"{BASE}/results/c42-confwords.json", "w"), indent=1)
    for src in ("word", "short"):
        z = [o for o in out if o["src"] == src and o["mean"] is not None and o["errors"] < o["refWords"]]
        m = np.array([o["mean"] for o in z]); print(src, len(z), "decoded with a right word; mean prob pcts 5/10/25/50", np.percentile(m, [5, 10, 25, 50]).round(3))
        for t in (0.6, 0.7, 0.75, 0.8):
            print(f"  t {t}: dropped {sum(1 for o in z if o['mean'] < t)}/{len(z)}", [o["text"] for o in z if o["mean"] < t][:6])


if __name__ == "__main__":
    main()
