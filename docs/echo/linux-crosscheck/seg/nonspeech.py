"""Cycle 42 (ADR 0129): does the gated segment peak (ADR 0127, helper `SegmentPeak`, landed e5818b3) make Parakeet
put words on non-speech? A dictation here is one LibriSpeech utterance, a 500 ms pause (the helper cuts there), then
1.5 s of a non-speech sound before a quick release (50 ms): a breath, key clicks, a lip/mouth click, room tone. The
sound is the release tail, so `PauseSegmenter.openHasSpeech` decides whether it is decoded, and the gate (peak < 0.1
-> peak 0.9, gain <= 10x) decides its level. Scored: words in the whole dictation beyond the reference (insertions),
how many tails got any words, gated vs plain. Levels are the sound's peak; speech stays at its LibriSpeech level
(normal mic) or x0.1 with the sound (quiet mic).
usage (from ~/echo-linux = /workspace/echo-linux): venv/bin/python seg/nonspeech.py [--n 60] [--kinds room] [--bg]
       [--out results/c42-ns.json];  python nonspeech.py selfcheck  (peak_gain = SegmentPeakTests.swift's cases)"""
import argparse, glob, json, os, random, sys
import numpy as np, soundfile as sf
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from helper_seg import segment_helper, Rule as HRule
from seg_eval import Decoder, norm_words, align_errors, BASE, SR

GATE, TARGET, CAP = 0.1, 0.9, 10.0
KINDS = ("breath", "keys", "click", "room")


def peak_gain(y):
    pk = float(np.max(np.abs(y))) if len(y) else 0.0
    return 1.0 if pk <= 0 or pk >= GATE else min(CAP, TARGET / pk)


def selfcheck():
    """The cases of SegmentPeakTests.testTheGain, so this harness gains exactly what the helper does."""
    g = lambda pk: peak_gain(np.array([pk, -pk / 2]))
    assert g(0.5) == 1 and g(0.1) == 1 and g(0.0) == 1, "normal, at the threshold, silence: untouched"
    assert abs(g(0.09) - 10) < 1e-5 and g(0.0001) == 10, "0.9 / 0.09 = 10; capped at 10x"
    assert abs(g(0.095) - 0.9 / 0.095) < 1e-6, "peak to 0.9"
    print("nonspeech selfcheck ok")


def bandnoise(rnd, n, lo, hi):
    X = np.fft.rfft(rnd.standard_normal(n)); f = np.fft.rfftfreq(n, 1 / SR)
    X[(f < lo) | (f > hi)] = 0; X[f > 0] /= np.sqrt(f[f > 0] / 300) ** 0.5  # a little pink
    return np.fft.irfft(X, n)


def sound(kind, rnd, roomtone, n=int(1.5 * SR)):
    y = np.zeros(n)
    if kind == "breath":  # 1-2 exhale/inhale bursts, 300-700 ms, 200-2500 Hz
        for _ in range(rnd.integers(1, 3)):
            L = int(rnd.uniform(0.3, 0.7) * SR); s = int(rnd.uniform(0, n - L))
            y[s:s + L] += bandnoise(rnd, L, 200, 2500) * np.hanning(L)
    elif kind == "keys":  # 3-8 key clicks: a 2-5 kHz ring decaying in ~8 ms plus a broadband transient
        for _ in range(rnd.integers(3, 9)):
            s = int(rnd.uniform(0, n - 800)); t = np.arange(800) / SR
            y[s:s + 800] += np.sin(2 * np.pi * rnd.uniform(2000, 5000) * t) * np.exp(-t / 0.008) + rnd.standard_normal(800) * np.exp(-t / 0.002)
    elif kind == "click":  # one or two mouth/lip clicks
        for _ in range(rnd.integers(1, 3)):
            s = int(rnd.uniform(0, n - 400)); t = np.arange(400) / SR
            y[s:s + 400] += rnd.standard_normal(400) * np.exp(-t / 0.004)
    elif kind == "room":  # real LibriSpeech room tone (the quiet ends of utterances), 10 ms crossfades
        out = np.zeros(0)
        while len(out) < n:
            c = roomtone[rnd.integers(len(roomtone))]; c = c * np.hanning(len(c)) ** 0.1
            out = np.concatenate([out, c])
        y = out[:n]
    return y / (np.max(np.abs(y)) + 1e-12)


def room_chunks(files):
    out = []
    for f in files:
        # the first 150 ms only when every 20 ms frame is quiet (< 0.002 RMS) and no sample tops 0.01: no word onset
        x, _ = sf.read(f, dtype="float32"); h = x[: int(0.15 * SR)].astype(np.float64)
        if len(h) == int(0.15 * SR) and np.max(np.abs(h)) < 0.01 and np.sqrt(np.mean(h[:2240].reshape(-1, 320) ** 2, axis=1)).max() < 0.002:
            out.append(h)
    return out


def main():
    p = argparse.ArgumentParser(); p.add_argument("--n", type=int, default=60); p.add_argument("--threads", type=int, default=6)
    p.add_argument("--out", default="results/c42-ns.json"); p.add_argument("--kinds", default=",".join(KINDS))
    p.add_argument("--bg", action="store_true", help="room tone under the whole dictation at the sound's level (the VAD floor sees the room)")
    a = p.parse_args()
    rnd = random.Random(42)
    files = sorted(glob.glob(f"{BASE}/data/LibriSpeech/test-clean/*/*/*.flac")); rnd.shuffle(files)
    room = room_chunks(files[:600]); refs = {}
    for ch in glob.glob(f"{BASE}/data/LibriSpeech/test-clean/*/*/*.trans.txt"):
        for l in open(ch): k, t = l.strip().split(" ", 1); refs[k] = t
    utts = []
    for f in files[600:]:
        x, _ = sf.read(f, dtype="float32")
        if 3 * SR <= len(x) <= 8 * SR: utts.append((os.path.basename(f)[:-5], x))
        if len(utts) >= a.n: break
    dec = Decoder(a.threads); rows = []; examples = []
    dec.cache_path = f"{os.path.dirname(os.path.abspath(__file__))}/cache-ns.json"  # not the seg_eval cache
    dec.cache = json.load(open(dec.cache_path)) if os.path.exists(dec.cache_path) else {}
    for mic, scale in (("normal", 1.0), ("quiet", 0.1)):
        for kind in a.kinds.split(","):
            for level in (0.005, 0.01, 0.02, 0.05, 0.09):
                agg = dict(mic=mic, kind=kind, peak=level, n=0, tailsDecoded=0, tailsGained=0)
                for gate in ("plain", "gated"):
                    agg[f"{gate}Errors"] = 0; agg[f"{gate}TailsWithWords"] = 0; agg[f"{gate}Insertions"] = 0
                words_ref = 0
                for i, (uid, u) in enumerate(utts):
                    # the same sound (per kind and dictation) at every level and on both mics, so rows compare
                    s = sound(kind, np.random.default_rng([42, KINDS.index(kind), i]), room) * level
                    x = np.concatenate([np.zeros(int(0.3 * SR)), u * scale, np.zeros(int(0.5 * SR)), s * scale, np.zeros(int(0.05 * SR))])
                    if a.bg:  # the room under everything; the tail is the room alone (plus the sound when it isn't room)
                        bgn = sound("room", np.random.default_rng([43, i]), room, len(x)) * level * scale
                        if kind == "room": x[: len(x) - len(s) - int(0.05 * SR)] += bgn[: len(x) - len(s) - int(0.05 * SR)]; x[-int(0.05 * SR):] += bgn[-int(0.05 * SR):]
                        else: x += bgn
                    x = x.astype(np.float32)
                    segs = segment_helper(x, HRule())
                    R = [w for w, _ in norm_words([(w, 0) for w in refs[uid].split()])]; words_ref += len(R); agg["n"] += 1
                    tail = segs[-1]; tail_dec = tail[3] and tail[0] >= int((0.3 * SR) + len(u) * 1.0)
                    agg["tailsDecoded"] += int(tail[3] and tail_dec)
                    noise_at = int(0.3 * SR) + len(u) + int(0.5 * SR)
                    ex = dict(mic=mic, kind=kind, peak=level, i=i, uid=uid)
                    for gate in ("plain", "gated"):
                        words = []
                        for (s0, s1, k, has) in segs:
                            if not has: continue
                            # speech-only ranges are the same audio for every kind and level: one cache entry
                            key = (f"ns:{mic}:{uid}" + (f":bg{level}" if a.bg else "")) if s1 <= noise_at else f"ns:{mic}:{kind}:{level}:{i}"
                            if gate == "gated" and (g := peak_gain(x[s0:s1])) != 1.0:
                                y = (x * g).astype(np.float32); kk = f"{key}:{s0}:{s1}:g{g:.4g}"
                                if (s0, s1) == tail[:2] and tail_dec: agg["tailsGained"] += 1
                            else: y = x; kk = f"{key}:{s0}:{s1}"
                            out = dec.words(kk, y, s0, s1, 0.64)
                            if s1 > noise_at: ex.setdefault(gate, []).append(dict(s0=round(s0 / SR, 2), s1=round(s1 / SR, 2), text=out["text"]))
                            if (s0, s1) == tail[:2] and tail_dec and out["words"]: agg[f"{gate}TailsWithWords"] += 1
                            words += out["words"]
                        H = [w for w, _ in norm_words([(w, t) for w, t in words])]
                        e, _ = align_errors(R, H); agg[f"{gate}Errors"] += e; agg[f"{gate}Insertions"] += max(0, len(H) - len(R))
                    if any(d["text"].strip() for g_ in ("plain", "gated") for d in ex.get(g_, [])): examples.append(ex)
                agg["words"] = words_ref
                for gate in ("plain", "gated"): agg[f"{gate}Wer"] = round(100 * agg[f"{gate}Errors"] / words_ref, 2)
                print(json.dumps(agg), flush=True); rows.append(agg)
        dec.save()
    os.makedirs(os.path.dirname(a.out) or ".", exist_ok=True); json.dump(dict(rows=rows, examples=examples), open(a.out, "w"), indent=1)


if __name__ == "__main__":
    selfcheck() if sys.argv[1:2] == ["selfcheck"] else main()
