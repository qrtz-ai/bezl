"""Cycle 38 (ADR 0120): sentence breaks after talk's rules cleanup, segmented vs whole-clip Parakeet text.
Writes /tmp/stitched.json (raw texts), runs cleanup.ts with bun: OLD_CLEANUP=<path to the previous cleanup.ts> NEW_CLEANUP=<repo>/packages/core/src/talk/cleanup.ts."""
import difflib, json, re, subprocess, sys, os
import soundfile as sf
sys.path.insert(0, os.path.dirname(__file__))
from seg_eval import ROOT, segment
cache = json.load(open(f"{ROOT}/cache.json")); dicts = json.load(open(f"{ROOT}/dictations.json"))
rows = []
for silence, cap in [(300, 15.0), (500, 15.0)]:
    for d in dicts:
        x, _ = sf.read(f"{ROOT}/wav/{d['name']}.wav", dtype="float32")
        S = []
        for a, b, k in segment(x, silence, cap, 2.0): S += [t for t, _ in cache[f"{d['name']}:{a}:{b}:0.64"]["words"]]
        W = [t for t, _ in cache[f"{d['name']}:0:{len(x)}:0.64"]["words"]]
        rows.append(dict(cfg=f"s{silence}-c{int(cap)}", seg=" ".join(S), whole=" ".join(W)))
json.dump(rows, open("/tmp/stitched.json", "w"))
js = """import {{ ruleCleanup }} from '{p}'; import fs from 'node:fs';
const r = JSON.parse(fs.readFileSync('/tmp/stitched.json','utf8'));
console.log(JSON.stringify(r.map((x: any) => ({{ ...x, seg: ruleCleanup(x.seg), whole: ruleCleanup(x.whole) }}))));"""
key = lambda w: re.sub(r"[^a-z0-9']", "", w.lower()); END = re.compile(r"[.?!]$")
def score(rs):
    out = {}
    for cfg in sorted({r["cfg"] for r in rs}):
        sp = lost = both = diff = n = 0
        for r in (x for x in rs if x["cfg"] == cfg):
            S, W = r["seg"].split(), r["whole"].split()
            m = difflib.SequenceMatcher(None, [key(t) for t in S], [key(t) for t in W], autojunk=False)
            for blk in m.get_matching_blocks():
                for i in range(blk.size):
                    se, we = bool(END.search(S[blk.a + i])), bool(END.search(W[blk.b + i]))
                    sp += se and not we; lost += we and not se; both += se and we
            sm = difflib.SequenceMatcher(None, S, W, autojunk=False)
            diff += sum(max(i2 - i1, j2 - j1) for op, i1, i2, j1, j2 in sm.get_opcodes() if op != "equal"); n += len(W)
        out[cfg] = dict(sentence_ends_agreeing=both, spurious=sp, lost=lost, formatted_diff_pct=round(100 * diff / n, 2))
    return out
res = {}
for name, p in [("raw", None), ("cleanup-old", os.environ.get("OLD_CLEANUP", "/tmp/cleanup_old.ts")), ("cleanup-new", os.environ["NEW_CLEANUP"])]:
    if p is None: rs = rows
    else:
        open("/tmp/run.ts", "w").write(js.format(p=p))
        rs = json.loads(subprocess.run(["bun", "/tmp/run.ts"], capture_output=True, text=True, check=True).stdout)
    res[name] = score(rs); print(name, json.dumps(res[name]))
json.dump(res, open(f"{ROOT}/results/punct-cleanup.json", "w"), indent=1)
