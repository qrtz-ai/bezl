# Paired comparison of two cross-check rows (results/<a>.json vs results/<b>.json), re-scored from their stored
# hypotheses against evalset.json: WER of each, the difference, and a seeded paired bootstrap over utterances
# (95 % interval of b - a, and how often b is not better). Tells a real change from the noise of 300 clips.
#   venv/bin/python compare.py results/a.json results/b.json [--n 2000]
#   venv/bin/python compare.py --selfcheck
import json, sys, argparse, os
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from score import norm, ed


def errors(path, refs):
    runs = json.load(open(path))["runs"]
    e = {r["id"]: ed(norm(refs[r["id"]]), norm(r["hyp"])) for r in runs}
    tail = {r["id"]: r["tailLost"] for r in runs}
    return e, tail


def paired(ea, eb, n_words, n=2000, seed=33):
    ids = sorted(set(ea) & set(eb)); A = np.array([ea[i] for i in ids], float); B = np.array([eb[i] for i in ids], float)
    W = np.array([n_words[i] for i in ids], float)
    rng = np.random.default_rng(seed); d = []
    for _ in range(n):
        k = rng.integers(0, len(ids), len(ids)); d.append(100 * (B[k].sum() - A[k].sum()) / W[k].sum())
    d = np.array(d)
    return dict(n=len(ids), a=100 * A.sum() / W.sum(), b=100 * B.sum() / W.sum(), lo=float(np.percentile(d, 2.5)),
                hi=float(np.percentile(d, 97.5)), pNotBetter=float(np.mean(d >= 0)),
                better=int((B < A).sum()), worse=int((B > A).sum()))


def selfcheck():
    W = {i: 20 for i in range(300)}
    a = {i: 1 for i in range(300)}
    same = paired(a, dict(a), W)
    assert same["lo"] == same["hi"] == 0 and same["better"] == same["worse"] == 0
    b = {i: (0 if i % 2 else 1) for i in range(300)}  # half the utterances lose their one error
    r = paired(a, b, W)
    assert abs(r["a"] - 5) < 1e-9 and abs(r["b"] - 2.5) < 1e-9 and r["hi"] < 0 and r["pNotBetter"] == 0, r
    assert ed("a b c".split(), "a x c d".split()) == 2 and norm("Twenty-five %") == ["25", "percent"]
    print("compare selfcheck ok")


if __name__ == "__main__":
    p = argparse.ArgumentParser(); p.add_argument("a", nargs="?"); p.add_argument("b", nargs="?")
    p.add_argument("--n", type=int, default=2000); p.add_argument("--evalset", default="evalset.json")
    p.add_argument("--selfcheck", action="store_true"); a = p.parse_args()
    if a.selfcheck: selfcheck(); sys.exit(0)
    refs = {i: ref for _, i, _, ref in json.load(open(a.evalset))}
    ea, ta = errors(a.a, refs); eb, tb = errors(a.b, refs)
    r = paired(ea, eb, {i: len(norm(refs[i])) for i in refs}, a.n)
    ids = sorted(set(ta) & set(tb))
    print(f"{os.path.basename(a.a)} {r['a']:.2f} % -> {os.path.basename(a.b)} {r['b']:.2f} %: "
          f"{r['b'] - r['a']:+.2f} pts ({100 * (r['b'] - r['a']) / max(r['a'], 1e-9):+.0f} % errors), "
          f"95 % CI [{r['lo']:+.2f}, {r['hi']:+.2f}], P(not better) {r['pNotBetter']:.3f}; "
          f"utterances better {r['better']} / worse {r['worse']} of {r['n']}; "
          f"last word missed {100 * sum(ta[i] for i in ids) / len(ids):.1f} -> {100 * sum(tb[i] for i in ids) / len(ids):.1f} %")
