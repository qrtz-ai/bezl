"""Cycle 42 (ADR 0131, N169): can the decoder's own token confidence tell a hallucinated release tail from a real
one? Parakeet TDT gives each emitted token a probability (sherpa-onnx `ys_log_probs`; FluidAudio's TDT decoder keeps
the same per-token softmax confidences and averages them into `ASRResult.confidence`). Tails:
  hallucinated : every noise-only tail nonspeech.py's runs put words on (strict room tone, --bg), plain and gated
  real         : the release tail (last helper-VAD segment) of seg_eval.py's 40 long dictations at x1, x0.1, x0.03
                 (gated peak as landed); short and one-word real tails are confwords.py
Rule scored: drop the tail's words when their mean token probability is below t (optionally only for tails of at
most W words). Reported: hallucinated tails dropped, real tails dropped and the reference words lost with them.
usage (from /workspace/echo-linux): venv/bin/python seg/confguard.py -> results/c42-confguard.json"""
import glob, json, os, random, sys
import numpy as np, soundfile as sf
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import nonspeech as N
from helper_seg import segment_helper, Rule as HRule
from seg_eval import norm_words, BASE, SR, ROOT


def decoder():
    import sherpa_onnx
    d = f"{BASE}/models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8"
    return sherpa_onnx.OfflineRecognizer.from_transducer(encoder=f"{d}/encoder.int8.onnx", decoder=f"{d}/decoder.int8.onnx", joiner=f"{d}/joiner.int8.onnx", tokens=f"{d}/tokens.txt", num_threads=6, model_type="nemo_transducer", feature_dim=128)


def decode(r, y):
    y = np.concatenate([y.astype(np.float32), np.zeros(int(0.64 * SR), np.float32)])
    s = r.create_stream(); s.accept_waveform(SR, y); r.decode_stream(s); res = s.result
    lp = list(res.ys_log_probs); return res.text, (float(np.mean(np.exp(lp))) if lp else None), (float(np.min(np.exp(lp))) if lp else None)


def main():
    r = decoder(); rnd = random.Random(42); out = dict(hall=[], real=[])
    files = sorted(glob.glob(f"{BASE}/data/LibriSpeech/test-clean/*/*/*.flac")); rnd.shuffle(files)
    room = N.room_chunks(files[:600]); utts = []
    for f in files[600:]:
        x, _ = sf.read(f, dtype="float32")
        if 3 * SR <= len(x) <= 8 * SR: utts.append((os.path.basename(f)[:-5], x))
        if len(utts) >= 60: break
    for bg, rf in ((False, "results/c42-ns-room.json"), (True, "results/c42-ns-bg.json")):
        for e in json.load(open(f"{BASE}/{rf}"))["examples"]:
            mic, kind, level, i = e["mic"], e["kind"], e["peak"], e["i"]; uid, u = utts[i]; scale = 1.0 if mic == "normal" else 0.1
            end = 0.3 + len(u) / SR
            s = N.sound(kind, np.random.default_rng([42, N.KINDS.index(kind), i]), room) * level
            x = np.concatenate([np.zeros(int(0.3 * SR)), u * scale, np.zeros(int(0.5 * SR)), s * scale, np.zeros(int(0.05 * SR))])
            if bg:
                bgn = N.sound("room", np.random.default_rng([43, i]), room, len(x)) * level * scale
                if kind == "room": x[: len(x) - len(s) - int(0.05 * SR)] += bgn[: len(x) - len(s) - int(0.05 * SR)]; x[-int(0.05 * SR):] += bgn[-int(0.05 * SR):]
                else: x += bgn
            x = x.astype(np.float32)
            for g in ("plain", "gated"):
                for d in e.get(g, []):
                    if not d["text"].strip() or d["s0"] < end - 0.02: continue
                    y = x[int(round(d["s0"] * SR)): int(round(d["s1"] * SR))]
                    if g == "gated": y = y * N.peak_gain(y)
                    text, mean, mn = decode(r, y)
                    if text.strip(): out["hall"].append(dict(bg=bg, gate=g, mic=mic, kind=kind, peak=level, i=i, text=text, mean=mean, min=mn, words=len(text.split())))
    dicts = json.load(open(f"{ROOT}/dictations.json"))
    for scale in (1.0, 0.1, 0.03):
        for dct in dicts:
            x, _ = sf.read(f"{ROOT}/wav/{dct['name']}.wav", dtype="float32"); x = (x * scale).astype(np.float32)
            hs = segment_helper(x, HRule()); a, b, _, has = hs[-1]
            if not has: continue
            y = x[a:b]; y = y * N.peak_gain(y)
            text, mean, mn = decode(r, y)
            if text.strip(): out["real"].append(dict(src="long", scale=scale, name=dct["name"], text=text, mean=mean, min=mn, words=len(text.split())))
    print("hall", len(out["hall"]), "real", len(out["real"]), flush=True)
    json.dump(out, open(f"{BASE}/results/c42-confguard.json", "w"), indent=1)


if __name__ == "__main__":
    main()
