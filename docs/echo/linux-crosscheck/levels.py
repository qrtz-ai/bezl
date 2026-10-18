# Calibration for the `talk latency` line's level fields (ADR 0112, dictation-tuning.md §10): the same meter as
# qrtz-talk's LevelMeter (block RMS x4 clamped to 1, as the helper's dictation.level; 1 dB bins; nearest-rank
# p10/p90 in dBFS) over the eval set's clips, 1365-sample blocks (one 85 ms tap block at 16 kHz), for each input
# condition, joined with that condition's per-utterance errors from results/<row>.json.
#   venv/bin/python levels.py [--selfcheck]
import json, sys, os, math
import numpy as np, soundfile as sf
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dsp
from score import norm, ed

BLOCK = 1365


def meter(x):
    """(p10, p90, blocks) like LevelMeter: floor(dBFS) bins from -100 to -12, nearest rank."""
    lv = []
    for k in range(0, len(x), BLOCK):
        b = x[k:k + BLOCK].astype(np.float64)
        level = min(1.0, math.sqrt(float(np.mean(b * b))) * 4)  # the helper's dictation.level
        rms = level / 4
        db = -100.0 if rms <= 0 else max(-100.0, 20 * math.log10(rms))
        lv.append(min(-12, max(-100, math.floor(db))))
    lv.sort(); n = len(lv)
    q = lambda p: lv[min(n, max(1, math.ceil(p * n))) - 1]
    return q(0.10), q(0.90), n


def selfcheck():
    x = np.concatenate([np.full(BLOCK * 3, 0.1, np.float32), np.full(BLOCK, 0.001, np.float32)] * 25)
    assert meter(x) == (-60, -20, 100), meter(x)  # the same case as tests/hardening36.rs
    print("levels selfcheck ok")


COND = {  # row name -> how run.py built its input
    "nem560-pad640": lambda x, i: x,
    "nem560-q01-pad640": lambda x, i: (x * 0.1).astype(np.float32),
    "nem560-wn15-pad640": lambda x, i: dsp.white_noise(x, 15.0, i),
}

if __name__ == "__main__":
    selfcheck()
    if "--selfcheck" in sys.argv: sys.exit()
    S = json.load(open("evalset.json")); refs = {i: ref for _, i, _, ref in S}
    for row, f in COND.items():
        path = f"results/{row}.json"
        if not os.path.exists(path): print(f"{row}: no results"); continue
        runs = {r["id"]: r for r in json.load(open(path))["runs"]}
        p10s, p90s = [], []; pooled = []
        for split, i, wav, ref in S:
            x, sr = sf.read(wav, dtype="float32"); y = f(x, i); p10, p90, _ = meter(y)
            p10s.append(p10); p90s.append(p90)
            R = norm(refs[i]); e = ed(R, norm(runs[i]["hyp"])); pooled.append((e, len(R)))
        print(f"{row}: p90 median {int(np.median(p90s))} dBFS (p10-p90 of clips {int(np.percentile(p90s,10))}..{int(np.percentile(p90s,90))}), "
              f"p10 median {int(np.median(p10s))}, spread median {int(np.median(np.array(p90s)-np.array(p10s)))} dB, "
              f"WER {100*sum(e for e,_ in pooled)/sum(n for _,n in pooled):.2f} %")
    # Per condition, not pooled buckets: across utterances, how hard the sentence is swamps the level (checked in
    # cycle 36: pooled WER by p90 bucket was not monotonic).
