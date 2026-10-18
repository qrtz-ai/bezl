"""Cycle 38: fix the punctuation at a pause cut from a short window decoded across it (helper proposal, ADR 0120).
For each cut c of the rule: decode [max(seg start, c - BEFORE), min(next end, c + AFTER)] + 640 ms pad, and take the
cut word's trailing punctuation and the next word's case from it. Compare with whole-clip text (raw, no cleanup)."""
import difflib, json, re, sys, os
import numpy as np, soundfile as sf
sys.path.insert(0, os.path.dirname(__file__))
from seg_eval import ROOT, SR, segment, Decoder
dec = Decoder(6); dicts = json.load(open(f"{ROOT}/dictations.json"))
key = lambda w: re.sub(r"[^a-z0-9']", "", w.lower()); END = re.compile(r"[.?!]$"); PUN = re.compile(r"[.?!,;:]+$")
BEFORE, AFTER = float(sys.argv[1]), float(sys.argv[2])
sp = lost = both = diff = n = fixed = 0; ms = []
for d in dicts:
    x, _ = sf.read(f"{ROOT}/wav/{d['name']}.wav", dtype="float32")
    segs = segment(x, 300, 15.0, 2.0); S = []; T = []; cuts = []
    for k, (a, b, kind) in enumerate(segs):
        w = dec.words(d["name"], x, a, b, 0.64)["words"]
        if k: cuts.append((len(S), a, segs[k - 1][0], b, kind))
        S += [t for t, _ in w]; T += [t for _, t in w]
    for j, c, a0, b1, _ in cuts:
        if j == 0 or j >= len(S): continue
        lo = max(a0, c - int(BEFORE * SR)); hi = min(b1, c + int(AFTER * SR))
        out = dec.words(d["name"], x, lo, hi, 0.64); ms.append(out["ms"])
        ww = out["words"]
        if not ww: continue
        # the window's word closest in time to S[j-1] (before c) and to S[j] (after c), matched by text
        bi = [i for i, (t, tt) in enumerate(ww) if tt < c / SR and key(t) == key(S[j - 1])]
        ai = [i for i, (t, tt) in enumerate(ww) if tt >= c / SR - 0.1 and key(t) == key(S[j])]
        if not bi or not ai or ai[0] != bi[-1] + 1: continue
        wb, wa = ww[bi[-1]][0], ww[ai[0]][0]
        nb = PUN.sub("", S[j - 1]) + (PUN.search(wb).group(0) if PUN.search(wb) else "")
        na = (S[j][0].upper() if wa[:1].isupper() else S[j][0].lower()) + S[j][1:]
        fixed += (nb != S[j - 1]) or (na != S[j]); S[j - 1], S[j] = nb, na
    dec.save()
    W = [t for t, _ in dec.words(d["name"], x, 0, len(x), 0.64)["words"]]
    m = difflib.SequenceMatcher(None, [key(t) for t in S], [key(t) for t in W], autojunk=False)
    for blk in m.get_matching_blocks():
        for i in range(blk.size):
            se, we = bool(END.search(S[blk.a + i])), bool(END.search(W[blk.b + i])); sp += se and not we; lost += we and not se; both += se and we
    sm = difflib.SequenceMatcher(None, S, W, autojunk=False)
    diff += sum(max(i2 - i1, j2 - j1) for op, i1, i2, j1, j2 in sm.get_opcodes() if op != "equal"); n += len(W)
r = dict(before=BEFORE, after=AFTER, sentence_ends_agreeing=both, spurious=sp, lost=lost, formatted_diff_pct=round(100 * diff / n, 2), cuts_changed=fixed,
         window_cpu_ms_p50=round(float(np.median(ms)), 1), window_cpu_ms_p90=round(float(np.percentile(ms, 90)), 1))
print(json.dumps(r)); json.dump(r, open(f"{ROOT}/results/cutwin-{BEFORE}-{AFTER}.json", "w"), indent=1)
