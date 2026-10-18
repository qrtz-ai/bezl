"""Cycle 42 (ADR 0131, N169): would a minimum of speech in the release tail stop Parakeet's words on non-speech
tails without losing a short last word? For each dictation the helper's PauseSegmenter (helper_seg.py) is fed the
whole clip; at the release its open segment's `speechFrames` (20 ms frames the VAD took for speech) is what a rule
`decode the tail only if speechFrames * 20 >= K ms` would look at (today K = 20: one frame, `openHasSpeech`).
Two sets:
  noise : nonspeech.py's dictations (utterance, pause, 1.5 s breath/keys/click/room), with and without room tone
          under everything (--bg), and which of their tails Parakeet put words on (from nonspeech.py's results)
  words : an utterance, a 500 ms pause, then ONE real word as the tail (the first word of another LibriSpeech
          utterance, cut at Parakeet's next-word timestamp), at the normal level and x0.1; scored: is the word
          decoded, and what speechFrames its tail has
usage (from /workspace/echo-linux): venv/bin/python seg/tailguard.py [--n 60] -> results/c42-tailguard.json"""
import argparse, glob, json, os, random, sys
import numpy as np, soundfile as sf
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import nonspeech as N
from helper_seg import PauseSegmenter, segment_helper, Rule as HRule
from seg_eval import Decoder, norm_words, BASE, SR

KS = (20, 60, 100, 140, 200, 260)


def tail_frames(x):
    s = PauseSegmenter(HRule()); s.feed(x); return s.speechFrames


def main():
    p = argparse.ArgumentParser(); p.add_argument("--n", type=int, default=60); p.add_argument("--threads", type=int, default=6)
    a = p.parse_args(); rnd = random.Random(42)
    files = sorted(glob.glob(f"{BASE}/data/LibriSpeech/test-clean/*/*/*.flac")); rnd.shuffle(files)
    room = N.room_chunks(files[:600]); utts = []
    for f in files[600:]:
        x, _ = sf.read(f, dtype="float32")
        if 3 * SR <= len(x) <= 8 * SR: utts.append((os.path.basename(f)[:-5], x))
        if len(utts) >= a.n: break
    out = dict(noise=[], words=[])
    # noise tails: rebuild nonspeech.py's dictations (same seeds) and read which tails got words from its results
    # (no-bg room rows from the strict room-tone re-run, c42-ns-room.json; the first run's room tone held onsets)
    for bg, res in ((False, ("results/c42-ns-a.json", "results/c42-ns-room.json")), (True, ("results/c42-ns-bg.json",))):
        ex = {}
        for rf in res:
            for e in json.load(open(f"{BASE}/{rf}"))["examples"]:
                if rf.endswith("-a.json") and e["kind"] == "room": continue
                ex[(e["mic"], e["kind"], e["peak"], e["i"])] = e
        for mic, scale in (("normal", 1.0), ("quiet", 0.1)):
            for kind in N.KINDS:
                for level in (0.005, 0.01, 0.02, 0.05, 0.09):
                    for i, (uid, u) in enumerate(utts):
                        s = N.sound(kind, np.random.default_rng([42, N.KINDS.index(kind), i]), room) * level
                        x = np.concatenate([np.zeros(int(0.3 * SR)), u * scale, np.zeros(int(0.5 * SR)), s * scale, np.zeros(int(0.05 * SR))])
                        if bg:
                            bgn = N.sound("room", np.random.default_rng([43, i]), room, len(x)) * level * scale
                            if kind == "room": x[: len(x) - len(s) - int(0.05 * SR)] += bgn[: len(x) - len(s) - int(0.05 * SR)]; x[-int(0.05 * SR):] += bgn[-int(0.05 * SR):]
                            else: x += bgn
                        x = x.astype(np.float32); segs = segment_helper(x, HRule()); t = segs[-1]
                        end = int(0.3 * SR) + len(u)
                        if not (t[3] and t[0] >= end): continue  # tail not decoded, or not the sound alone
                        e = ex.get((mic, kind, level, i), {}); words = {}
                        for g in ("plain", "gated"):
                            words[g] = any(d["text"].strip() and d["s0"] >= end / SR - 0.02 for d in e.get(g, []))
                        out["noise"].append(dict(bg=bg, mic=mic, kind=kind, peak=level, i=i, frames=tail_frames(x), **{f"{g}Words": words[g] for g in words}))
    # word tails
    dec = Decoder(a.threads); dec.cache = {}; dec.save = lambda: None
    donors = []
    for f in files[600 + 200:]:
        x, _ = sf.read(f, dtype="float32")
        if len(x) > 12 * SR: continue
        r = dec.words("d" + f, x, 0, len(x), 0.64)
        w = r["words"]
        if len(w) < 3: continue
        t0, t1 = w[0][1], w[1][1]
        a0 = max(0, int((t0 - 0.12) * SR)); a1 = int((t1 - 0.04) * SR)  # TDT stamps a word's first token: cut before the next
        word = norm_words([(w[0][0], 0)])
        if not word or a1 - a0 < int(0.12 * SR): continue
        donors.append((word[0][0], x[a0:a1]))
        if len(donors) >= a.n: break
    for mic, scale in (("normal", 1.0), ("quiet", 0.1)):
        for i, (uid, u) in enumerate(utts[: len(donors)]):
            wref, wav = donors[i]
            x = np.concatenate([np.zeros(int(0.3 * SR)), u * scale, np.zeros(int(0.5 * SR)), wav * scale, np.zeros(int(0.05 * SR))]).astype(np.float32)
            segs = segment_helper(x, HRule()); t = segs[-1]; end = int(0.3 * SR) + len(u)
            alone = t[0] >= end
            got = dec.words(f"w{mic}{i}", x, t[0], t[1], 0.64)["text"] if t[3] else ""
            hyp = [w for w, _ in norm_words([(got, 0)])]
            out["words"].append(dict(mic=mic, i=i, word=wref, ms=round(1000 * len(wav) / SR), alone=alone, decoded=bool(t[3]), frames=tail_frames(x), hyp=got, hit=wref in hyp))
    # the rule at each K
    summ = []
    for K in KS:
        f = K // 20
        r = dict(K=K)
        for bg in (False, True):
            n = [z for z in out["noise"] if z["bg"] == bg]
            tag = "bg" if bg else "nobg"
            r[f"{tag}NoiseTails"] = len(n)
            for g in ("plain", "gated"):
                r[f"{tag}{g.capitalize()}Hallucinated"] = sum(1 for z in n if z[f"{g}Words"])
                r[f"{tag}{g.capitalize()}HallucinatedAfterRule"] = sum(1 for z in n if z[f"{g}Words"] and z["frames"] >= f)
        for mic in ("normal", "quiet"):
            w = [z for z in out["words"] if z["mic"] == mic and z["alone"]]
            r[f"{mic}WordTails"] = len(w); r[f"{mic}WordsHit"] = sum(1 for z in w if z["hit"])
            r[f"{mic}WordsHitAfterRule"] = sum(1 for z in w if z["hit"] and z["frames"] >= f)
        summ.append(r); print(json.dumps(r), flush=True)
    out["rule"] = summ
    json.dump(out, open(f"{BASE}/results/c42-tailguard.json", "w"), indent=1)


if __name__ == "__main__":
    main()
