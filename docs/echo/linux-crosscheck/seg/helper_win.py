"""Cycle 39: independent check of the helper's cut windows as landed (faff542, ADR 0120, SegmentedTranscriber):
pause cuts only (cap cuts skipped), the segment before the cut has speech, window [max(previous cut, c - 3 s),
c + 2 s] + 640 ms pad, queued only once c + 2 s is captured; at the release, a window not decoded is skipped.
Repunctuation as WordStitcher.repunctuate. Compared with cutwin.py's idealised windows (every cut, window cut short
at the next segment's end) and with a release window for the skipped last cut. Scored raw and through talk's
ruleCleanup (bun) on both sides against the whole-clip decode, as punct_cleanup.py.
usage: helper_win.py <cleanup.ts>"""
import difflib, json, re, subprocess, sys, os
import numpy as np, soundfile as sf
sys.path.insert(0, os.path.dirname(__file__))
from seg_eval import ROOT, SR, segment, Decoder
dec = Decoder(6); dicts = json.load(open(f"{ROOT}/dictations.json"))
key = lambda w: re.sub(r"[^a-z0-9']", "", w.lower()); END = re.compile(r"[.?!]$"); PUN = re.compile(r"[.?!,;:]+$")
B, A, PAD = 3.0, 2.0, 0.64

def repunct(S, T, c, ww):
    """WordStitcher.repunctuate: S words, T global starts (1 ms grid), c cut seconds, ww window [(w, t)]."""
    c = round(c, 3); j = next((i for i, t in enumerate(T) if t >= c), None)
    if j is None: return "notReady"
    if j == 0: return "noMatch"
    kb, ka = key(S[j - 1]), key(S[j])
    if not kb or not ka: return "noMatch"
    bi = [i for i, (t, tt) in enumerate(ww) if round(tt, 3) < c and key(t) == kb]
    ai = [i for i, (t, tt) in enumerate(ww) if round(tt, 3) >= c - 0.1 and key(t) == ka]
    if not bi or not ai or ai[0] != bi[-1] + 1: return "noMatch"
    wb, wa = ww[bi[-1]][0], ww[ai[0]][0]
    nb = PUN.sub("", S[j - 1]) + (PUN.search(wb).group(0) if PUN.search(wb) else "")
    na = ((S[j][0].upper() if wa[:1].isupper() else S[j][0].lower()) + S[j][1:]) if S[j] and wa else S[j]
    if nb == S[j - 1] and na == S[j]: return "unchanged"
    S[j - 1], S[j] = nb, na; return "changed"

rows = []; stats = {}
for v in ["seg", "helper", "helper+cap", "helper+release", "cutwin"]:
    st = stats[v] = dict(pause_cuts=0, cap_cuts=0, windows=0, fixes=0, no_match=0, skipped_near_release=0, skipped_cap=0, skipped_no_speech=0, release_ms=[])
    for d in dicts:
        x, _ = sf.read(f"{ROOT}/wav/{d['name']}.wav", dtype="float32"); n = len(x)
        segs = segment(x, 300, 15.0, 2.0); S = []; T = []; cuts = []
        for k, (a, b, kind) in enumerate(segs):
            w = dec.words(d["name"], x, a, b, PAD)["words"]
            if k: cuts.append((a, segs[k - 1][0], b, segs[k - 1][2], bool(prev_w)))
            prev_w = w; S += [t for t, _ in w]; T += [round(tt, 3) for _, tt in w]
        last_rel = 0.0
        for c, prev, nxt_end, kind, speech in cuts:
            st["cap_cuts" if kind == "cap" else "pause_cuts"] += 1
            if v == "seg": continue
            lo = max(prev, c - int(B * SR))
            if v == "cutwin": hi = min(nxt_end, c + int(A * SR))
            else:
                if kind == "cap" and v != "helper+cap": st["skipped_cap"] += 1; continue
                if not speech: st["skipped_no_speech"] += 1; continue
                hi = c + int(A * SR)
                if hi > n:
                    if v != "helper+release": st["skipped_near_release"] += 1; continue
                    hi = n
            out = dec.words(d["name"], x, lo, hi, PAD); st["windows"] += 1
            if v == "helper+release" and c + int(A * SR) > n: st["release_ms"].append(out["ms"])
            r = repunct(S, T, c / SR, out["words"]) if out["words"] else "noMatch"
            st["fixes"] += r == "changed"; st["no_match"] += r in ("noMatch", "notReady")
        W = [t for t, _ in dec.words(d["name"], x, 0, n, PAD)["words"]]
        rows.append(dict(cfg=v, seg=" ".join(S), whole=" ".join(W)))
    dec.save()
    rm = st.pop("release_ms"); st["release_windows"] = len(rm)
    if rm: st["release_window_cpu_ms_p50_p90_max"] = [round(float(np.median(rm))), round(float(np.percentile(rm, 90))), round(max(rm))]

def score(rs):
    out = {}
    for cfg in dict.fromkeys(r["cfg"] for r in rs):
        sp = lost = both = diff = nw = sps = 0
        for r in (x for x in rs if x["cfg"] == cfg):
            S, W = r["seg"].split(), r["whole"].split()
            m = difflib.SequenceMatcher(None, [key(t) for t in S], [key(t) for t in W], autojunk=False)
            for blk in m.get_matching_blocks():
                for i in range(blk.size):
                    se, we = bool(END.search(S[blk.a + i])), bool(END.search(W[blk.b + i])); sp += se and not we; lost += we and not se; both += se and we
                    # strict: the whole clip clearly continues (its next word is lowercase); a capitalized next word
                    # with no mark is the whole-clip decode dropping a sentence end (long-clip punctuation loss)
                    nx = W[blk.b + i + 1] if blk.b + i + 1 < len(W) else ""
                    sps += se and not we and nx[:1].islower()
            sm = difflib.SequenceMatcher(None, S, W, autojunk=False)
            diff += sum(max(i2 - i1, j2 - j1) for op, i1, i2, j1, j2 in sm.get_opcodes() if op != "equal"); nw += len(W)
        out[cfg] = dict(agreeing=both, spurious=sp, spurious_whole_continues=sps, lost=lost, formatted_diff_pct=round(100 * diff / nw, 2))
    return out
json.dump(rows, open("/tmp/qrtz-sc-c39/stitched.json", "w"))
open("/tmp/qrtz-sc-c39/run.ts", "w").write(f"""import {{ ruleCleanup }} from '{sys.argv[1]}'; import fs from 'node:fs';
const r = JSON.parse(fs.readFileSync('/tmp/qrtz-sc-c39/stitched.json','utf8'));
console.log(JSON.stringify(r.map((x: any) => ({{ ...x, seg: ruleCleanup(x.seg), whole: ruleCleanup(x.whole) }}))));""")
cl = json.loads(subprocess.run(["bun", "/tmp/qrtz-sc-c39/run.ts"], capture_output=True, text=True, check=True).stdout)
res = dict(stats=stats, raw=score(rows), cleanup=score(cl))
print(json.dumps(res, indent=1)); json.dump(res, open(f"{ROOT}/results/helper-win.json", "w"), indent=1)
