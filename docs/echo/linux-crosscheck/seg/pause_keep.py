"""Cycle 40 (ADR 0123): the sentence ends the cut windows lose (ADR 0121: 13 -> 23 through the cleanup). At a cut the
window decides the cut word's mark; where it says "," (or nothing) and the segment's own decode said ".", is the
pause length a safe tie-breaker? The helper knows the whole pause when the window is queued (2 s after the cut).
Replays helper_win.py's "helper+cap" variant (as landed in 1c73921) from cached decodes, measures the silence run
around each cut with the segmenter's own streaming detector, and sweeps rules applied after
WordStitcher.repunctuate:
  keep>=P: the window gave no end but the segment did, and the pause is >= P ms -> keep the segment's mark and case
  drop<Q : the window gave an end but the pause is < Q ms -> no end (the window's mark minus .?!), lowercase next
Scored raw and through talk's ruleCleanup on both sides against the whole-clip decode, as helper_win.py.
usage: pause_keep.py <cleanup.ts>"""
import difflib, json, re, subprocess, sys, os
import numpy as np, soundfile as sf
sys.path.insert(0, os.path.dirname(__file__))
from seg_eval import ROOT, SR, FRAME, segment, Decoder, frame_rms, Silence
dec = Decoder(6); dicts = json.load(open(f"{ROOT}/dictations.json"))
key = lambda w: re.sub(r"[^a-z0-9']", "", w.lower()); END = re.compile(r"[.?!]$"); PUN = re.compile(r"[.?!,;:]+$")
B, A, PAD = 3.0, 2.0, 0.64
TMP = "/tmp/qrtz-sc-c40"

def pause_ms(sil, c):
    """Silence run (20 ms frames, streaming detector flags) containing sample c, in ms."""
    f = min(c // FRAME, len(sil) - 1); a = b = f
    while a > 0 and sil[a - 1]: a -= 1
    while b + 1 < len(sil) and sil[b + 1]: b += 1
    return 20 * (b - a + 1) if sil[f] else 0

def repunct(S, T, c, ww, seg_mark):
    """WordStitcher.repunctuate -> (result, j, window mark, window next-upper)."""
    c = round(c, 3); j = next((i for i, t in enumerate(T) if t >= c), None)
    if j is None or j == 0: return "noMatch", j, None, None
    kb, ka = key(S[j - 1]), key(S[j])
    if not kb or not ka: return "noMatch", j, None, None
    bi = [i for i, (t, tt) in enumerate(ww) if round(tt, 3) < c and key(t) == kb]
    ai = [i for i, (t, tt) in enumerate(ww) if round(tt, 3) >= c - 0.1 and key(t) == ka]
    if not bi or not ai or ai[0] != bi[-1] + 1: return "noMatch", j, None, None
    wb, wa = ww[bi[-1]][0], ww[ai[0]][0]
    m = PUN.search(wb).group(0) if PUN.search(wb) else ""
    return "ok", j, m, wa[:1].isupper()

def setmark(S, j, mark, upper):
    S[j - 1] = PUN.sub("", S[j - 1]) + mark
    if S[j]: S[j] = (S[j][0].upper() if upper else S[j][0].lower()) + S[j][1:]

# one replay, recording every applicable cut; rules are applied afterwards on copies
clips = []; cutlog = []
for d in dicts:
    x, _ = sf.read(f"{ROOT}/wav/{d['name']}.wav", dtype="float32"); n = len(x)
    det = Silence(); sil = np.array([det.silent(r) for r in frame_rms(x)])
    segs = segment(x, 300, 15.0, 2.0); S = []; T = []; cuts = []; prev_w = None
    for k, (a, b, kind) in enumerate(segs):
        w = dec.words(d["name"], x, a, b, PAD)["words"]
        if k: cuts.append((a, segs[k - 1][0], segs[k - 1][2], bool(prev_w)))
        prev_w = w; S += [t for t, _ in w]; T += [round(tt, 3) for _, tt in w]
    seg0 = list(S); ops = []
    for c, prev, kind, speech in cuts:
        if not speech or c + int(A * SR) > n: continue
        out = dec.words(d["name"], x, max(prev, c - int(B * SR)), c + int(A * SR), PAD)
        if not out["words"]: continue
        r, j, m, up = repunct(S, T, c / SR, out["words"], None)
        if r != "ok": continue
        segm = PUN.search(seg0[j - 1]).group(0) if PUN.search(seg0[j - 1]) else ""
        ops.append(dict(j=j, win_mark=m, win_upper=up, seg_mark=segm, seg_upper=seg0[j][:1].isupper(), pause=pause_ms(sil, c), kind=kind))
    W = [t for t, _ in dec.words(d["name"], x, 0, n, PAD)["words"]]
    clips.append(dict(S=seg0, W=W, ops=ops))
    for o in ops:
        # what the whole clip has at the cut word (aligned by key), for the per-cut table
        cutlog.append(dict(o, name=d["name"]))

def apply(rule, P):
    rows = []; changed = 0
    for cl in clips:
        S = list(cl["S"])
        for o in cl["ops"]:
            m, up = o["win_mark"], o["win_upper"]
            if rule == "keep" and not re.search(r"[.?!]", m) and re.search(r"[.?!]", o["seg_mark"]) and o["pause"] >= P:
                m, up = o["seg_mark"], o["seg_upper"]; changed += 1
            if rule == "drop" and re.search(r"[.?!]", m) and o["pause"] < P:
                m, up = re.sub(r"[.?!]", "", m) or ",", False; changed += 1
            setmark(S, o["j"], m, up)
        rows.append(dict(seg=" ".join(S), whole=" ".join(cl["W"])))
    return rows, changed

def score(rs):
    sp = lost = both = diff = nw = sps = 0
    for r in rs:
        S, W = r["seg"].split(), r["whole"].split()
        m = difflib.SequenceMatcher(None, [key(t) for t in S], [key(t) for t in W], autojunk=False)
        for blk in m.get_matching_blocks():
            for i in range(blk.size):
                se, we = bool(END.search(S[blk.a + i])), bool(END.search(W[blk.b + i])); sp += se and not we; lost += we and not se; both += se and we
                nx = W[blk.b + i + 1] if blk.b + i + 1 < len(W) else ""
                sps += se and not we and nx[:1].islower()
        sm = difflib.SequenceMatcher(None, S, W, autojunk=False)
        diff += sum(max(i2 - i1, j2 - j1) for op, i1, i2, j1, j2 in sm.get_opcodes() if op != "equal"); nw += len(W)
    return dict(agreeing=both, spurious=sp, strict=sps, lost=lost, diff_pct=round(100 * diff / nw, 2))

variants = [("asLanded", None)] + [("keep", p) for p in (400, 500, 600, 700, 800, 1000)] + [("drop", q) for q in (340, 400, 500)]
allrows = []; res = {}
for rule, P in variants:
    rows, ch = apply(rule, P if P is not None else 0)
    name = rule if P is None else f"{rule}{'>=' if rule == 'keep' else '<'}{P}"
    res[name] = dict(changed=ch, raw=score(rows)); allrows += [dict(r, cfg=name) for r in rows]
os.makedirs(TMP, exist_ok=True); json.dump(allrows, open(f"{TMP}/rows.json", "w"))
open(f"{TMP}/run.ts", "w").write(f"""import {{ ruleCleanup }} from '{sys.argv[1]}'; import fs from 'node:fs';
const r = JSON.parse(fs.readFileSync('{TMP}/rows.json','utf8'));
console.log(JSON.stringify(r.map((x: any) => ({{ ...x, seg: ruleCleanup(x.seg), whole: ruleCleanup(x.whole) }}))));""")
cl = json.loads(subprocess.run(["bun", f"{TMP}/run.ts"], capture_output=True, text=True, check=True).stdout)
for name in res: res[name]["cleanup"] = score([r for r in cl if r["cfg"] == name])
# pause distribution of the window's decisions
P = np.array([o["pause"] for o in cutlog]); win_end = np.array([bool(re.search(r"[.?!]", o["win_mark"])) for o in cutlog])
seg_end = np.array([bool(re.search(r"[.?!]", o["seg_mark"])) for o in cutlog])
res["cuts"] = dict(n=len(cutlog), pause_p10_p50_p90=[int(np.percentile(P, q)) for q in (10, 50, 90)],
    window_end_by_pause={f"{lo}-{hi}": [int(((P >= lo) & (P < hi) & win_end).sum()), int(((P >= lo) & (P < hi)).sum())] for lo, hi in ((0, 400), (400, 600), (600, 800), (800, 1000), (1000, 5000))},
    seg_end_window_none=int((seg_end & ~win_end).sum()))
print(json.dumps(res, indent=1)); json.dump(res, open(f"{ROOT}/results/pause-keep.json", "w"), indent=1)
