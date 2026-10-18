"""Cycle 44 (ADR 0134): the helper VAD's noise floor in a noisy room. PauseSegmenter starts every dictation with
floor 0.001 (it is re-made per dictation) and lets it rise only 0.2 % per 20 ms frame (x10 in ~23 s), capped at
speech level x0.1. In a room whose noise is above ~0.001 x 2.5 (8 dB margin), the noise counts as speech until the
floor catches up, so a dictation gets no pause cuts for its first seconds: only cap cuts, and a long tail decoded at
release. This replays the 40 long dictations (seg/dictations.json) with room noise under everything (0.3 s of it
before the first word too: the user presses, then talks), at SNRs against the dictation's active speech level, and
compares floor rules:
  shipped   floor = 0.9 f + 0.1 r if r < f else min(f * 1.002, speech * 0.1)
  creep     the same with x1.01 per frame (x10 in ~4.6 s)
  creepN    x1.00N per frame (creep005: x1.005)
  minstat   shipped, plus: once 1 s of frames is seen, the floor rises to the minimum of the 100 ms-smoothed RMS
            over the last 1 s whenever that is higher (still capped at speech x0.1); a room is learned within ~1 s,
            and a gap between words in that second is what the minimum finds (minstatMS: an MS window)
Scored per rule and SNR: pause / cap cuts a dictation, cuts landing inside a word (clean signal within +-40 ms above
-25 dB of the speech level), release tail audio (s, p50/p90), its CPU decode ms, WER (Parakeet TDT v2 int8, sherpa).
Result (ADR 0134): both faster floors cut more and cost accuracy everywhere; the shipped floor stays.
usage (from /workspace/echo-linux): venv/bin/python seg/roomfloor.py [--snr 99,30,20,15] [--noise room,fan]
       [--rules shipped,creep,minstat] [--nodecode] [--out results/c44-roomfloor.json]
       python seg/roomfloor.py selfcheck   (shipped == helper_seg.PauseSegmenter; minstat learns a room in ~1 s)"""
import argparse, glob, json, os, sys
import numpy as np, soundfile as sf
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from helper_seg import PauseSegmenter, Rule as HRule, FRAME, segment_helper
from seg_eval import Decoder, norm_words, align_errors, frame_rms, BASE, ROOT, SR
from nonspeech import room_chunks, bandnoise


class Seg(PauseSegmenter):
    def __init__(self, rule=None, mode="shipped", creep=1.002, win_ms=1000):
        super().__init__(rule); self.mode = mode; self.creep = creep; self.win = win_ms // 20; self.hist = []

    def frame_done(self, f):
        # the shipped update happens inside super(); replace it by pre-computing the floor the base would set and
        # correcting it before is_speech runs: re-implemented here line for line instead (helper_seg order).
        r = self.rms(f); ru = self.rule
        rise = self.creep if self.mode == "creep" else 1.002
        self.floor = 0.9 * self.floor + 0.1 * r if r < self.floor else min(self.floor * rise, self.speechLevel * 0.1)
        if self.mode == "minstat":
            self.hist.append(r)
            if len(self.hist) > self.win + 4: self.hist.pop(0)
            if len(self.hist) >= self.win + 4:
                h = np.convolve(self.hist, np.ones(5) / 5, "valid")[-self.win:]
                m = float(h.min())
                if m > self.floor: self.floor = min(m, self.speechLevel * 0.1)
        # the rest of PauseSegmenter.frame_done, unchanged
        speech = self.is_speech(r); self.position += len(f)
        self.frameRms.append(r); self.frameSpeech.append(speech)
        if speech: self.silenceRun = 0; self.speechFrames += 1; self.speechLevel = 0.95 * self.speechLevel + 0.05 * r
        else: self.silenceRun += 1
        ms = 20
        if (ru.pauseMs is not None and self.speechFrames * ms >= ru.minSpeechMs and self.silenceRun * ms >= ru.pauseMs
                and (self.position - (self.silenceRun // 2) * FRAME - self.segmentStart) // 16 >= ru.minSegmentMs):
            return self.cut(self.position - (self.silenceRun // 2) * FRAME, False)
        if ru.capMs is not None and (self.position - self.segmentStart) // 16 >= max(1_000, ru.capMs - ru.segmentPadMs):
            n = len(self.frameRms); look = max(1, min(n - 1, ru.capSearchMs // ms))
            def smooth(k):
                lo, hi = max(0, k - 2), min(n - 1, k + 2)
                return sum(self.frameRms[lo:hi + 1]) / (hi - lo + 1)
            best, bestE = n - 1, smooth(n - 1)
            for k in range(n - 1, n - look - 1, -1):
                e = smooth(k)
                if e < bestE: best, bestE = k, e
            return self.cut(self.segmentStart + best * FRAME, True)
        return None


def rule_mode(rule):
    """shipped | creep (x1.01) | creepN (x1.00N, e.g. creep005) | minstat (1 s window) | minstatMS (e.g. minstat2000)"""
    if rule.startswith("creep"): return "creep", dict(creep=1 + float("0." + rule[5:]) if rule[5:] else 1.01)
    if rule.startswith("minstat") and rule[7:]: return "minstat", dict(win_ms=int(rule[7:]))
    return rule, {}


def segment(x, mode, **kw):
    s = Seg(HRule(), mode, **kw); out = []; s0 = 0
    for at, cap, had in s.feed(x):
        out.append((s0, at, "cap" if cap else "pause", had)); s0 = at
    out.append((s0, len(x), "end", s.open_has_speech()))
    return [c for c in out if c[1] > c[0]], s


def selfcheck():
    rnd = np.random.default_rng(1)
    x = np.concatenate([np.zeros(4800), rnd.standard_normal(16000 * 4) * 0.05, np.zeros(8000), rnd.standard_normal(16000 * 3) * 0.05]).astype(np.float32)
    a = segment_helper(x); b, _ = segment(x, "shipped")
    assert a == b, (a, b)
    # speech bursts (0.05 RMS, 400 ms, 150 ms gaps) over a room at 0.005: after 6 s the shipped floor is still
    # ~0.0018 (x1.002 a frame from 0.001), minstat and creep (x1.01) sit at the room (capped at speech x0.1)
    sp = np.concatenate([np.concatenate([np.full(6400, 1.0), np.zeros(2400)]) for _ in range(11)])
    room = (rnd.standard_normal(len(sp)) * 0.005 + rnd.standard_normal(len(sp)) * 0.05 * sp).astype(np.float32)
    _, s1 = segment(room, "shipped"); _, s2 = segment(room, "minstat"); _, s3 = segment(room, "creep", creep=1.01)
    assert s1.floor < 0.0035 and s2.floor > 0.0045 and s3.floor > 0.0045, (s1.floor, s2.floor, s3.floor)
    print("roomfloor selfcheck ok", round(s1.floor, 5), round(s2.floor, 5), round(s3.floor, 5))


def active_rms(x):
    e = frame_rms(x); th = np.percentile(e, 95) * 10 ** (-30 / 20)
    return float(np.sqrt(np.mean(e[e > th] ** 2)))


def noise(kind, n, seed, room):
    rnd = np.random.default_rng(seed)
    if kind == "fan":
        y = bandnoise(rnd, n, 60, 6000)
    else:  # real LibriSpeech room tone, each 150 ms chunk RMS-normalised, crossfaded
        out = []; L = 0
        while L < n:
            c = room[rnd.integers(len(room))]; c = c / (np.sqrt(np.mean(c ** 2)) + 1e-12) * np.hanning(len(c)) ** 0.1
            out.append(c); L += len(c)
        y = np.concatenate(out)[:n]
    return y / (np.sqrt(np.mean(y ** 2)) + 1e-12)


def main():
    p = argparse.ArgumentParser(); p.add_argument("--snr", default="99,30,20,15"); p.add_argument("--noise", default="room,fan")
    p.add_argument("--rules", default="shipped,creep,minstat"); p.add_argument("--nodecode", action="store_true")
    p.add_argument("--threads", type=int, default=6); p.add_argument("--out", default="results/c44-roomfloor.json")
    a = p.parse_args()
    dicts = json.load(open(f"{ROOT}/dictations.json"))
    files = sorted(glob.glob(f"{BASE}/data/LibriSpeech/test-clean/*/*/*.flac")); np.random.default_rng(0)
    room = room_chunks(files[:600])
    dec = None
    if not a.nodecode:
        dec = Decoder(a.threads); dec.cache_path = f"{ROOT}/cache-roomfloor.json"
        dec.cache = json.load(open(dec.cache_path)) if os.path.exists(dec.cache_path) else {}
    clean = {d["name"]: sf.read(f"{ROOT}/wav/{d['name']}.wav", dtype="float32")[0] for d in dicts}
    rows = []
    for kind in a.noise.split(","):
        for snr in [float(s) for s in a.snr.split(",")]:
            if snr >= 99 and kind != a.noise.split(",")[0]: continue  # clean once
            for rule in a.rules.split(","):
                agg = dict(noise=kind if snr < 99 else "clean", snr=snr, rule=rule, pause=0, cap=0, inWord=0, firstPauseS=[], tailS=[], tailMs=[], E=0, N=0)
                for i, d in enumerate(dicts):
                    x0 = clean[d["name"]]; sp = active_rms(x0)
                    x = x0.astype(np.float64)
                    if snr < 99: x = x + noise(kind, len(x), [44, i], room) * sp * 10 ** (-snr / 20)
                    x = x.astype(np.float32)
                    mode, kw = rule_mode(rule)
                    segs, _ = segment(x, mode, **kw)
                    e0 = frame_rms(x0); thw = sp * 10 ** (-25 / 20)
                    for (s0, s1, k, has) in segs[:-1]:
                        agg[k] += 1; f = s1 // FRAME
                        if e0[max(0, f - 2):f + 2].max() > thw: agg["inWord"] += 1
                    pc = [s1 for (s0, s1, k, h) in segs if k == "pause"]
                    agg["firstPauseS"].append(pc[0] / SR if pc else d["seconds"])
                    t0, t1 = segs[-1][0], segs[-1][1]; agg["tailS"].append((t1 - t0) / SR)
                    if dec:
                        key = f"rf:{d['name']}:{agg['noise']}:{snr}"; words = []
                        for (s0, s1, k, has) in segs:
                            if not has: continue
                            out = dec.words(key, x, s0, s1, 0.64); words += out["words"]
                            if k == "end": agg["tailMs"].append(out["ms"])
                        R = [w for w, _ in norm_words([(w, 0) for w in d["ref"].split()])]
                        H = [w for w, _ in norm_words([(w, t) for w, t in words])]
                        e, _ = align_errors(R, H); agg["E"] += e; agg["N"] += len(R)
                q = lambda v, p: round(float(np.percentile(v, p)), 2) if v else None
                n = len(dicts)
                row = dict(noise=agg["noise"], snr=snr, rule=rule, pausePer=round(agg["pause"] / n, 2), capPer=round(agg["cap"] / n, 2),
                           inWord=agg["inWord"], firstPauseS=dict(p50=q(agg["firstPauseS"], 50), p90=q(agg["firstPauseS"], 90)),
                           tailS=dict(p50=q(agg["tailS"], 50), p90=q(agg["tailS"], 90), max=q(agg["tailS"], 100)),
                           tailMs=dict(p50=q(agg["tailMs"], 50), p90=q(agg["tailMs"], 90)) if agg["tailMs"] else None,
                           wer=round(100 * agg["E"] / agg["N"], 2) if agg["N"] else None)
                print(json.dumps(row), flush=True); rows.append(row)
            if dec: dec.save()
    os.makedirs(os.path.dirname(a.out) or ".", exist_ok=True); json.dump(dict(rows=rows), open(a.out, "w"), indent=1)


if __name__ == "__main__":
    selfcheck() if sys.argv[1:2] == ["selfcheck"] else main()
