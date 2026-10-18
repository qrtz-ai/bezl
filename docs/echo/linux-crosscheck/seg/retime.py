"""Re-time only the release decode (last segment + pad) for a few configs, back to back, on a quieter box."""
import json, sys, time, numpy as np, soundfile as sf
sys.argv = ["x"]; sys.path.insert(0, "."); import seg_eval as S
dec = S.Decoder(6); dicts = json.load(open("dictations.json")); out = {}
cfgs = {"whole": None, "s300-c15-o0": (300, 15), "s300-c10-o0": (300, 10), "s300-c8-o0": (300, 8)}
for name, c in cfgs.items():
    ms = []; secs = []
    for d in dicts:
        x, _ = sf.read(f"wav/{d['name']}.wav", dtype="float32")
        a, b, _k = (0, len(x), "end") if c is None else S.segment(x, c[0], c[1], 2.0)[-1]
        y = np.concatenate([x[a:b], np.zeros(int(0.64 * 16000), np.float32)])
        t = time.perf_counter(); s = dec.r.create_stream(); s.accept_waveform(16000, y); dec.r.decode_stream(s); ms.append((time.perf_counter() - t) * 1000); secs.append(len(y) / 16000)
    q = lambda v, p: round(float(np.percentile(v, p)))
    out[name] = dict(p50=q(ms, 50), p90=q(ms, 90), max=q(ms, 100), rtf=round(sum(ms) / 1000 / sum(secs), 3))
    print(name, out[name], flush=True)
json.dump(out, open("retime.json", "w"), indent=1)
