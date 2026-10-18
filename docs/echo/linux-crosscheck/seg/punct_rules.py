"""Stitch rules for the '.' Parakeet writes at a segment's end (cycle 38). Cached decodes only."""
import difflib, json, re, sys, os
import numpy as np, soundfile as sf
sys.path.insert(0, os.path.dirname(__file__))
from seg_eval import ROOT, segment
cache = json.load(open(f"{ROOT}/cache.json")); dicts = json.load(open(f"{ROOT}/dictations.json"))
key = lambda w: re.sub(r"[^a-z0-9']", "", w.lower())
END = re.compile(r"[.?!]$")
def lone_dot(w): return w.endswith(".") and not w.endswith("..") and "." not in w[:-1]
def apply(rule, before, after):
    """-> (before, after) at one cut."""
    if rule == "none": return before, after
    if rule in ("lower", "lower+cap") and lone_dot(before) and after[:1].islower(): return before[:-1], after
    return before, after
def cleanup_caps(S):  # what talk's SENTENCE_START then does: capitalize after .?!
    out = []
    for i, w in enumerate(S):
        if i and END.search(out[-1]) and w[:1].islower(): w = w[0].upper() + w[1:]
        out.append(w)
    return out
for silence, cap in [(300, 15.0), (500, 15.0)]:
    for rule in ["none", "lower"]:
        tp = fp = fn = tn = 0; diff = n = 0
        for d in dicts:
            x, _ = sf.read(f"{ROOT}/wav/{d['name']}.wav", dtype="float32")
            segs = segment(x, silence, cap, 2.0); S = []; cuts = []
            for k, (a, b, kind) in enumerate(segs):
                w = [t for t, _ in cache[f"{d['name']}:{a}:{b}:0.64"]["words"]]
                if k and S and w:
                    S[-1], w[0] = apply(rule, S[-1], w[0]); cuts.append(len(S))
                S += w
            S = cleanup_caps(S)
            W = cleanup_caps([t for t, _ in cache[f"{d['name']}:0:{len(x)}:0.64"]["words"]])
            m = difflib.SequenceMatcher(None, [key(t) for t in S], [key(t) for t in W], autojunk=False); mp = {}
            for blk in m.get_matching_blocks():
                for i in range(blk.size): mp[blk.a + i] = blk.b + i
            for j in cuts:
                if j - 1 not in mp: continue
                se, we = bool(END.search(S[j - 1])), bool(END.search(W[mp[j - 1]]))
                tp += se and we; fp += se and not we; fn += we and not se; tn += (not se) and (not we)
            sm = difflib.SequenceMatcher(None, S, W, autojunk=False)
            diff += sum(max(i2 - i1, j2 - j1) for op, i1, i2, j1, j2 in sm.get_opcodes() if op != "equal"); n += len(W)
        print(json.dumps(dict(silence=silence, cap=cap, rule=rule, breaks_kept=tp, spurious=fp, lost=fn, none_both=tn,
                              formatted_diff_pct=round(100 * diff / n, 2))))
