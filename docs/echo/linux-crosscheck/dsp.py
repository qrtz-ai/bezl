# Input-side DSP for the Echo Linux cross-check (dictation-tuning.md §4 items 5 and 7, ADR 0104). Dev-box only.
#
# - swift_linear: an exact port of the helper's DSP.Resampler (native/qrtz-helper/Sources/QrtzHelperCore/
#   Audio.swift:68-90), fed in the tap's 4096-frame blocks (mac/Audio.swift:78) with its state across blocks.
#   Linear interpolation, no low-pass. At 48 kHz the step is exactly 3.0, so it is a plain decimator: out[n] = x[3n].
# - sinc: a band-limited 48 -> 16 kHz (241-tap Kaiser windowed sinc, ~86 dB stopband from 8 kHz), standing in for
#   AVAudioConverter at mastering quality.
# - up48: the high-quality 16 -> 48 kHz used to make a "mic at 48 kHz" signal from LibriSpeech.
# - white_noise: seeded white noise at an SNR against the clip's RMS (added at 48 kHz, so 2/3 of it is above 8 kHz),
#   or only 8-20 kHz (band="hf": all of it is out of the model's band).
# - peak_norm (whole clip, an oracle) and agc (causal, per tap block, peak hold): the quiet-mic normalizers.
#
# `python dsp.py` runs the self-checks (no model needed).
import numpy as np
from scipy import signal

TAP = 4096  # mac/Audio.swift:78 installTap bufferSize


def swift_linear_ref(x, src, dst, block=TAP):
    """Literal, slow port of DSP.Resampler.process, one block at a time (the reference for the self-check)."""
    pos, last, out, step = 0.0, np.float32(0), [], src / dst
    for k in range(0, len(x), block):
        c = x[k:k + block]
        if len(c) == 0:
            continue
        while pos < len(c) - 1:
            i = int(np.floor(pos)); fr = np.float32(pos - i)
            a = last if i < 0 else c[i]; b = c[i + 1]
            out.append(np.float32(a + (b - a) * fr)); pos += step
        pos -= len(c); last = c[-1]
    return np.array(out, np.float32)


def swift_linear(x, src, dst, block=TAP):
    """Vectorised DSP.Resampler, same blocks and state. Positions are pos + k*step instead of repeated `pos += step`
    (identical when step is an integer, as at 48 kHz; ~1e-12 apart otherwise)."""
    x = np.asarray(x, np.float32); step = src / dst; pos, last, out = 0.0, np.float32(0), []
    for k in range(0, len(x), block):
        c = x[k:k + block]; n = len(c)
        if n == 0:
            continue
        if pos < n - 1:
            p = pos + step * np.arange(int((n - 1 - pos) / step) + 2)
            p = p[p < n - 1]
            i = np.floor(p).astype(np.int64); fr = (p - i).astype(np.float32)
            a = np.where(i < 0, last, c[np.maximum(i, 0)]); b = c[i + 1]
            out.append((a + (b - a) * fr).astype(np.float32))
            pos = p[-1] + step
        pos -= n; last = c[-1]
    return np.concatenate(out) if out else np.zeros(0, np.float32)


_H = signal.firwin(241, 7600, window=("kaiser", 8.6), fs=48000)


def sinc(x48):
    return signal.resample_poly(np.asarray(x48, np.float64), 1, 3, window=_H).astype(np.float32)


def up48(x16):
    return signal.resample_poly(np.asarray(x16, np.float64), 3, 1, window=_H).astype(np.float32)


def seed_of(key):
    return int(key) if str(key).isdigit() else sum(map(ord, str(key))) * 1_000_003 + len(str(key))


_HF = signal.butter(8, [8000, 20000], btype="band", fs=48000, output="sos")


def white_noise(x, snr_db, key, band=None):
    """band=None: white over the whole band. band="hf" (48 kHz input only): 8-20 kHz only, i.e. nothing the model
    should hear (hiss, fans, the top of sibilants). The sinc path removes ~23 dB of it (the 8th-order Butterworth's
    skirt leaks just under 8 kHz); the linear path keeps all of it, folded into 0-8 kHz."""
    rng = np.random.default_rng(seed_of(key))
    n = rng.standard_normal(len(x)).astype(np.float32)
    if band == "hf":
        n = signal.sosfilt(_HF, n).astype(np.float32)
        n /= max(1e-12, float(np.sqrt(np.mean(np.square(n, dtype=np.float64)))))
    rms = float(np.sqrt(np.mean(np.square(x, dtype=np.float64))))
    return (x + n * (rms / 10 ** (snr_db / 20))).astype(np.float32)


def hf_band_via48(x16, snr_db, key, resampler):
    """The `--hf` rows of dictation-tuning.md §7, exactly as they were run on the box: up with scipy's default
    polyphase filter, 8-20 kHz noise at snr_db against the 48 kHz clip's RMS (seed: the sum of the id's characters),
    back to 16 kHz with the linear decimator ("linear") or scipy's default polyphase filter ("poly")."""
    up = signal.resample_poly(x16, 3, 1).astype(np.float32)
    rng = np.random.default_rng(int(key) if str(key).isdigit() else sum(map(ord, str(key))))
    n = signal.sosfilt(_HF, rng.standard_normal(len(up)).astype(np.float32)).astype(np.float32)
    rms = np.sqrt(np.mean(up ** 2)); n *= rms / (10 ** (snr_db / 20)) / max(1e-9, np.sqrt(np.mean(n ** 2))); up = up + n
    if resampler == "linear":
        t = np.arange(0, len(up) - 1, 3.0); i = t.astype(int); fr = (t - i).astype(np.float32)
        return (up[i] + (up[i + 1] - up[i]) * fr).astype(np.float32)
    return signal.resample_poly(up, 1, 3).astype(np.float32)


def head_drop(x, sr, drop_ms):
    """A mic that opens after the user starts talking (§7 `--headdrop`): leading silence cut to 20 ms before the
    first 20 ms frame above 5 % of the clip's peak frame RMS, then drop_ms more dropped."""
    f = int(sr * 0.02); e = [np.sqrt(np.mean(x[i:i + f] ** 2)) for i in range(0, len(x) - f, f)]
    th = max(e) * 0.05; first = min([i for i, v in enumerate(e) if v > th] + [0])
    return x[max(0, (first - 1) * f) + int(sr * drop_ms / 1000):]


def peak_norm(x, target=0.9, cap=10.0):
    """Whole-clip peak normalisation (needs the whole clip: an oracle for a batch final, not for streaming)."""
    pk = float(np.max(np.abs(x))) if len(x) else 0.0
    return (x * min(cap, target / max(pk, 1e-9))).astype(np.float32)


def agc(x, target=0.9, cap=10.0, block=1365):
    """Causal per-block normaliser, the shape the helper could run per tap block: the gain is
    min(cap, target / peak so far, this block included), so it only ever falls (peak hold for one dictation, no
    release) and never clips: a block whose peak lowers the gain gets the new gain at once (instant attack; a ramp
    from the old gain would overshoot the target in that block). Starts at `cap`: silence before speech is boosted
    by at most cap."""
    x = np.asarray(x, np.float32); out = np.empty_like(x); peak = 0.0
    for k in range(0, len(x), block):
        c = x[k:k + block]
        peak = max(peak, float(np.max(np.abs(c))))
        g1 = min(cap, target / max(peak, 1e-9))
        out[k:k + len(c)] = c * g1
    return out


def _tone(f, sr, s=1.0, amp=0.5):
    return (amp * np.sin(2 * np.pi * f * np.arange(int(sr * s)) / sr)).astype(np.float32)


def _db(y):
    return 20 * np.log10(max(1e-12, float(np.sqrt(np.mean(np.square(y[2000:-2000], dtype=np.float64))))))


def selfcheck():
    rng = np.random.default_rng(1)
    for src in (48000, 44100, 24000):
        x = rng.standard_normal(src // 2 + 777).astype(np.float32) * 0.3
        a, b = swift_linear_ref(x, src, 16000), swift_linear(x, src, 16000)
        assert len(a) == len(b) and np.max(np.abs(a - b)) < 1e-5, (src, len(a), len(b))
    x = rng.standard_normal(48000 + 5).astype(np.float32)
    y = swift_linear(x, 48000, 16000)
    assert np.array_equal(y, x[0:3 * len(y):3]), "48 kHz linear is x[3n]"
    # a 12 kHz tone (mic hiss, sibilants) folds to 4 kHz at full level through the linear path; sinc removes it
    t = _tone(12000, 48000)
    lin12, sin12 = _db(swift_linear(t, 48000, 16000)), _db(sinc(t))
    assert lin12 > -10 and sin12 < -70, (lin12, sin12)
    f = np.abs(np.fft.rfft(swift_linear(t, 48000, 16000)))
    assert abs(np.argmax(f) * 16000 / (2 * (len(f) - 1)) - 4000) < 5, "aliases to 4 kHz"
    # 1 kHz passes both within 0.1 dB; the 16 -> 48 -> 16 round trip keeps a 3 kHz tone
    p = _tone(1000, 48000)
    assert abs(_db(sinc(p)) - _db(p)) < 0.1 and abs(_db(swift_linear(p, 48000, 16000)) - _db(p)) < 0.1
    q = _tone(3000, 16000)
    assert np.max(np.abs(sinc(up48(q))[500:-500] - q[500:-500])) < 2e-3
    # noise level and seeding
    s = _tone(500, 16000, amp=0.1)
    n = white_noise(s, 15, "1089-134686-0000") - s
    assert abs(_db(s) - _db(n) - 15) < 0.2 and np.array_equal(n, white_noise(s, 15, "1089-134686-0000") - s)
    # 8-20 kHz noise at 48 kHz: at the SNR, and the sinc path removes it (> 60 dB) while linear keeps it in band
    s48 = up48(_tone(500, 16000, amp=0.1)); h = white_noise(s48, 15, "k", band="hf") - s48
    assert abs(_db(s48) - _db(h) - 15) < 0.2
    assert _db(sinc(h)) < _db(h) - 20 and _db(swift_linear(h, 48000, 16000)) > _db(h) - 1
    # normalisers: target peak, gain cap, causal never clips and only falls
    v = _tone(300, 16000, amp=0.2)
    assert abs(np.max(np.abs(peak_norm(v))) - 0.9) < 1e-3
    assert abs(np.max(np.abs(peak_norm(v * 0.01))) - 0.02) < 1e-4  # capped at 10x
    w = np.concatenate([np.zeros(8000, np.float32) + 1e-4, v, 2 * v]).astype(np.float32)
    o = agc(w)
    assert np.max(np.abs(o)) <= 0.9 + 1e-6 and abs(np.max(np.abs(o[8000:24000])) - 0.9) < 0.02
    assert abs(np.max(np.abs(o[:8000])) - 1e-3) < 1e-6  # leading near-silence: at most cap
    print("dsp selfcheck ok: 48k linear = x[3n]; 12 kHz tone linear %.1f dB vs sinc %.1f dB (aliases to 4 kHz)" % (lin12, sin12))


if __name__ == "__main__":
    selfcheck()
