"""Formatting at segment cuts (cycle 38): does segmenting add sentence breaks the whole-clip decode doesn't have?
Uses only cached decodes (cache.json). For each cut of the chosen rule, compare the segmented text with the
whole-clip decode at the aligned position: a '.?!' on the word before the cut, and a capital on the word after."""
import difflib, json, re, sys
import numpy as np, soundfile as sf
sys.path.insert(0, __import__("os").path.dirname(__file__))
from seg_eval import ROOT, SR, segment

cache = json.load(open(f"{ROOT}/cache.json")); dicts = json.load(open(f"{ROOT}/dictations.json"))
key = lambda w: re.sub(r"[^a-z0-9']", "", w.lower())
END = re.compile(r"[.?!]$")
PROPER = set()
def run(silence=300, cap=15.0, minseg=2.0, pad=0.64):
    st = dict(cuts=0, utt=0, mid=0, aligned=0, seg_end=0, whole_end=0, spurious=0, missing=0, spurious_utt=0, spurious_mid=0,
              seg_cap=0, whole_cap=0, cap_spurious=0, comma_lost=0, fwer_seg=0, fwer_whole=0, n=0, examples=[])
    for d in dicts:
        x, _ = sf.read(f"{ROOT}/wav/{d['name']}.wav", dtype="float32")
        segs = segment(x, silence, cap, minseg)
        S = []; cutidx = []
        for k, (a, b, kind) in enumerate(segs):
            w = cache[f"{d['name']}:{a}:{b}:{pad}"]["words"]
            if k: cutidx.append((len(S), a, segs[k - 1][2]))
            S += [t for t, _ in w]
        W = [t for t, _ in cache[f"{d['name']}:0:{len(x)}:{pad}"]["words"]]
        m = difflib.SequenceMatcher(None, [key(t) for t in S], [key(t) for t in W], autojunk=False)
        mp = {}
        for blk in m.get_matching_blocks():
            for i in range(blk.size): mp[blk.a + i] = blk.b + i
        for j, a, kind in cutidx:
            st["cuts"] += 1
            z = x[max(0, a - 160): a + 160]; utt = bool(np.all(z == 0)); st["utt" if utt else "mid"] += 1
            if j - 1 not in mp or j not in mp: continue
            st["aligned"] += 1
            sb, wb = S[j - 1], W[mp[j - 1]]; sa, wa = S[j], W[mp[j]]
            se, we = bool(END.search(sb)), bool(END.search(wb))
            st["seg_end"] += se; st["whole_end"] += we
            if se and not we:
                st["spurious"] += 1; st["spurious_utt" if utt else "spurious_mid"] += 1
                if len(st["examples"]) < 12: st["examples"].append(f"{'utt' if utt else 'mid'} {kind}: seg '{' '.join(S[j-4:j+2])}' | whole '{' '.join(W[mp[j-1]-3:mp[j]+2])}'")
            if we and not se: st["missing"] += 1
            if wb.endswith(",") and not sb.endswith(","): st["comma_lost"] += 1
            sc, wc = sa[:1].isupper(), wa[:1].isupper(); st["seg_cap"] += sc; st["whole_cap"] += wc
            if sc and not wc: st["cap_spurious"] += 1
        # formatted (case + punctuation) word error rate of the segmented text against the whole-clip text
        sm = difflib.SequenceMatcher(None, S, W, autojunk=False)
        st["fwer_seg"] += sum(max(i2 - i1, j2 - j1) for op, i1, i2, j1, j2 in sm.get_opcodes() if op != "equal"); st["n"] += len(W)
    st["formatted_diff_pct"] = round(100 * st.pop("fwer_seg") / st["n"], 2); st.pop("fwer_whole")
    return st
if __name__ == "__main__":
    for cfg in [(300, 15.0, 2.0), (500, 15.0, 2.0), (300, 10.0, 2.0)]:
        try: r = run(*cfg)
        except KeyError as e: print(cfg, "not cached", e); continue
        ex = r.pop("examples"); print(cfg, json.dumps(r))
        if cfg == (300, 15.0, 2.0): print("\n".join(ex))
