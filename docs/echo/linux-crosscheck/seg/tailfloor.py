"""Cycle 43 (ADR 0131 as landed in 63aab4c, N169/N170): replay the helper's release tail confidence floor on Linux.
Mirrors, line by line:
  SegmentedTranscriber.dropsTail (QrtzHelperCore/Segmented.swift): floor set, a confidence reported, conf < floor
    (strict), and 1 <= whitespace-split words of the tail decode's text <= SegmentRule.tailMaxWords (4)
  SegmentRule.fromEnvironment, QRTZ_ECHO_TAIL_CONF: trimmed, lowercased; "off"/"0" -> nil; Float(v) with 0 < f <= 1
  FluidAudio 0.17.7 AsrManager.calculateConfidence (what ParakeetBatch passes on as BatchResult.confidence): empty
    text -> 0.1; token confidences missing or not one per token -> 0.5; else the mean clamped to [0.1, 1]
Sets: cycle 42's results (c42-confguard.json: hallucinated noise-only tails and the 40 long dictations' real release
tails, gated; c42-confwords.json: one-word and short-reply tails; c42-ns-room/-bg.json rows for WER), plus
`--ungated`: the quiet real tails re-decoded WITHOUT the gated peak (the Mac default: QRTZ_ECHO_SEG_PEAK off), which
cycle 42 measured only gained.
usage (from /workspace/echo-linux): venv/bin/python seg/tailfloor.py [--ungated] -> results/c43-tailfloor.json
       python tailfloor.py selfcheck   (TailConfTests.swift's cases)"""
import glob, json, math, os, random, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
TAIL_MAX_WORDS = 4


def drops_tail(text, conf, floor):
    """Segmented.swift dropsTail."""
    if floor is None or conf is None or not conf < floor: return False
    n = len(text.split())
    return 0 < n <= TAIL_MAX_WORDS


def floor_from_env(v):
    """SegmentRule.fromEnvironment's QRTZ_ECHO_TAIL_CONF branch (Swift Float(String): 'nan', 'inf' parse)."""
    if v is None: return None
    v = v.strip().lower()
    if not v or v in ("off", "0"): return None
    try: f = float(v)
    except ValueError: return None
    return f if f > 0 and f <= 1 else None


def fluid_conf(text, probs):
    """FluidAudio calculateConfidence: the value ParakeetBatch reports (never NaN: clamped)."""
    if not text.strip(): return 0.1
    if not probs: return 0.5
    m = float(np.mean(probs))
    return max(0.1, min(1.0, m)) if not math.isnan(m) else 1.0


def selfcheck():
    # TailConfTests.swift
    assert not drops_tail("w9", 0.2, None), "off is a no-op"
    assert drops_tail("w9 w10", 0.4, 0.6) and drops_tail("w9 w10 w11 w12", 0.59, 0.6), "<= 4 words under the floor"
    assert not drops_tail("w9 w10 w11 w12 w13", 0.4, 0.6) and not drops_tail("w9", 0.8, 0.6), "long or sure: kept"
    assert not drops_tail("w9", None, 0.6) and not drops_tail("", 0.1, 0.6), "no confidence, or no words: kept"
    assert not drops_tail("w9", 0.6, 0.6), "strict <"
    assert floor_from_env("0.6") == 0.6 and floor_from_env("1") == 1 and floor_from_env(" 0.7 ") == 0.7
    for bad in ("off", "0", "-0.2", "1.5", "x", "nan", "inf", "", "on"): assert floor_from_env(bad) is None, bad
    # FluidAudio's sentinels: an empty decode reports 0.1 (kept: no words), missing token confidences 0.5 (dropped!)
    assert fluid_conf("", []) == 0.1 and not drops_tail("", fluid_conf("", []), 0.6)
    assert fluid_conf("Yes.", []) == 0.5 and drops_tail("Yes.", fluid_conf("Yes.", []), 0.6)
    print("tailfloor selfcheck ok")


def ungated(out_path):
    """The quiet real tails without the gated peak: confguard's long release tails at x0.1 / x0.03 and confwords'
    one-word and short tails on the quiet mic, decoded plain (sherpa-onnx Parakeet TDT v2 int8)."""
    import soundfile as sf
    import nonspeech as N
    from confguard import decoder, decode
    from helper_seg import segment_helper, Rule as HRule
    from seg_eval import norm_words, align_errors, BASE, SR, ROOT
    r = decoder(); out = []
    for scale in (0.1, 0.03):
        for dct in json.load(open(f"{ROOT}/dictations.json")):
            x, _ = sf.read(f"{ROOT}/wav/{dct['name']}.wav", dtype="float32"); x = (x * scale).astype(np.float32)
            a, b, _, has = segment_helper(x, HRule())[-1]
            if not has: continue
            y = x[a:b]; g = N.peak_gain(y)
            text, mean, mn = decode(r, y)
            out.append(dict(src="long", scale=scale, name=dct["name"], gain=g, text=text, mean=mean, words=len(text.split())))
    # confwords.py's tails, same donors and order
    rnd = random.Random(42)
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
    for f in files[800:]:
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
    for scale in (0.1, 0.03):
        for j, (src, ref, wav) in enumerate(tails):
            uid, u = utts[j % len(utts)]
            x = np.concatenate([np.zeros(int(0.3 * SR)), u * scale, np.zeros(int(0.5 * SR)), wav * scale, np.zeros(int(0.05 * SR))]).astype(np.float32)
            a, b, _, has = segment_helper(x, HRule())[-1]
            if not has or a < int(0.3 * SR) + len(u): continue
            y = x[a:b]; text, mean, mn = decode(r, y)
            R = [w for w, _ in norm_words([(w, 0) for w in ref.split()])]; H = [w for w, _ in norm_words([(text, 0)])]
            e, _ = align_errors(R, H)
            out.append(dict(src=src, scale=scale, ref=ref, gain=N.peak_gain(y), text=text, mean=mean, refWords=len(R), errors=e))
    json.dump(out, open(out_path, "w"), indent=1)
    return out


def replay(base, ungated_rows):
    from seg_eval import norm_words
    g = json.load(open(f"{base}/results/c42-confguard.json")); w = json.load(open(f"{base}/results/c42-confwords.json"))
    nw = lambda t: len(norm_words([(t, 0)]))
    lt = lambda m, fl: m is not None and m < fl
    hall = g["hall"]; real_long = g["real"]
    right = [o for o in w if o["mean"] is not None and o["errors"] < o["refWords"]]
    res = dict(floors={})
    for fl in (0.5, 0.6, 0.7, 0.75):
        d = dict()
        d["hallDropped"] = sum(drops_tail(h["text"], h["mean"], fl) for h in hall)
        d["hallUnderFloorButLong"] = sum(lt(h["mean"], fl) and h["words"] > TAIL_MAX_WORDS for h in hall)
        d["hallWordsRemoved"] = sum(nw(h["text"]) for h in hall if drops_tail(h["text"], h["mean"], fl))
        d["hallWords"] = sum(nw(h["text"]) for h in hall)
        d["realLongDropped"] = sum(drops_tail(o["text"], o["mean"], fl) for o in real_long)
        d["realLongUnder"] = sum(lt(o["mean"], fl) for o in real_long)
        for src in ("word", "short"):
            z = [o for o in right if o["src"] == src]
            d[f"real_{src}"] = [sum(drops_tail(o["text"], o["mean"], fl) for o in z), len(z)]
        if ungated_rows:
            for src in ("long", "word", "short"):
                z = [o for o in ungated_rows if o["src"] == src and o["mean"] is not None and o.get("errors", 0) < o.get("refWords", 1)]
                d[f"ungated_{src}"] = [sum(drops_tail(o["text"], o["mean"], fl) for o in z), len(z),
                                       [o["text"] for o in z if drops_tail(o["text"], o["mean"], fl)][:6]]
        # WER on the non-speech sets: a dropped phantom tail removes its words (all insertions at the end)
        for name in ("room", "bg"):
            ns = json.load(open(f"{base}/results/c42-ns-{name}.json"))
            for gate in ("plain", "gated"):
                words = sum(r["words"] for r in ns["rows"]); errs = sum(r[f"{gate}Errors"] for r in ns["rows"])
                rem = sum(nw(h["text"]) for h in hall if h["bg"] == (name == "bg") and h["gate"] == gate and drops_tail(h["text"], h["mean"], fl))
                d[f"wer_{name}_{gate}"] = [round(100 * errs / words, 2), round(100 * (errs - rem) / words, 2)]
        res["floors"][str(fl)] = d
    res["hallTotal"] = len(hall); res["realLongTotal"] = len(real_long)
    res["hallLongTails"] = sum(h["words"] > TAIL_MAX_WORDS for h in hall)
    return res


if __name__ == "__main__":
    if sys.argv[1:2] == ["selfcheck"]: selfcheck(); sys.exit()
    selfcheck()
    from seg_eval import BASE
    up = f"{BASE}/results/c43-ungated.json"
    rows = ungated(up) if "--ungated" in sys.argv else (json.load(open(up)) if os.path.exists(up) else None)
    res = replay(BASE, rows)
    json.dump(res, open(f"{BASE}/results/c43-tailfloor.json", "w"), indent=1)
    print(json.dumps(res, indent=1))
