# Echo Linux cross-check

Reproduces the table in [dictation-tuning.md §6](../dictation-tuning.md#6-linux-cross-check-same-model-family-onnx)
(ADR 0102). Dev-box tooling only. It is not shipped, the app never runs it, and nothing here goes over the network at
run time: the models and the audio are local files that you fetch once.

What it measures: the same model family as the Mac (NVIDIA Nemotron Speech Streaming 0.6B and Parakeet TDT 0.6B v2,
int8 ONNX exports via [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)) on 300 seeded LibriSpeech utterances.
WER is comparable to FluidAudio's published numbers. CPU timings are not Mac Neural Engine timings; only their
order carries over.

## Setup (once)

```bash
mkdir -p ~/echo-linux && cd ~/echo-linux && cp <repo>/docs/echo/linux-crosscheck/*.{py,sh} .
python3 -m venv venv && venv/bin/pip install sherpa-onnx soundfile numpy scipy  # sherpa-onnx 1.13.8, scipy 1.18.1 were used
# models/: the four sherpa-onnx release archives (github.com/k2-fsa/sherpa-onnx/releases, tag asr-models)
#   sherpa-onnx-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25
#   sherpa-onnx-nemotron-speech-streaming-en-0.6b-1120ms-int8-2026-04-25
#   sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11
#   sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8
# data/LibriSpeech/test-clean, data/LibriSpeech/test-other: openslr.org/12 (test-clean.tar.gz, test-other.tar.gz)
venv/bin/python evalset.py                                                    # -> evalset.json (300 utterances)
```

## Run

`bash run-all.sh` runs every row, or one at a time:

```bash
venv/bin/python run.py models/<set> --name <row> [--pad S] [--trim [--trimkeep S]] [--offline] [--chunkpad N] [--threads N]
```

- `--pad S`: seconds of zeros fed after the audio before `input_finished` (the helper's `padMs`; default 0.64).
- `--trim`: cut the clip to `--trimkeep` (default 0.04 s) after the last 20 ms frame above 5 % of the clip's peak
  RMS. This is a quick release, the key let go right after the last word. `--trimkeep 0.15` is a 150 ms tail.
- `--chunkpad N`: pad the remainder to a multiple of N samples (what FluidAudio's `finish()` does with 8960).
- `--offline`: a batch recogniser over the whole utterance (the two-pass final).
- `--via48 linear|sinc` (cycle 33, ADR 0104): the clip as a 48 kHz mic. Up to 48 kHz with a band-limited filter
  (`dsp.up48`), then back to 16 kHz either through `dsp.swift_linear`, an exact port of the helper's
  `DSP.Resampler` fed in the tap's 4096-frame blocks (at 48 kHz it is a plain decimator, `x[3n]`, no low-pass), or
  through `dsp.sinc` (241-tap Kaiser windowed sinc, ~86 dB stopband from 8 kHz: what `AVAudioConverter` does).
- `--noise DB [--band hf]`: seeded white noise at DB SNR against the clip's RMS, per utterance. With `--via48` it is
  added at 48 kHz (two thirds of it above 8 kHz); `--band hf` keeps only 8-20 kHz, i.e. nothing the model should
  hear (hiss, fans, the top of sibilants).
- `--scale F [--norm peak|agc]`: a quiet mic (`--scale 0.1`), then optionally normalised: `peak` scales the whole
  clip to a 0.9 peak (an oracle: needs the whole clip), `agc` is causal per 85 ms block, gain = min(10, 0.9 / peak
  so far), the shape the helper could run (`dsp.agc`).
- `--lang L`: the per-stream language option (Nemotron 3.5: `en`, `auto`).

`python dsp.py` and `python compare.py --selfcheck` check the DSP (the Swift port against a literal loop, the
aliasing of a 12 kHz tone, the noise level, the normalisers) and the comparison without a model.
`python compare.py results/a.json results/b.json` re-scores two rows from their hypotheses and gives the WER change
with a 95 % paired bootstrap interval over utterances, so a change can be told from the noise of 300 clips.

## Fields (`results/<row>.json` → `summary`)

- `werAll`, `wer[split]`: word error rate in %. Both sides are lowercased, punctuation is dropped, and number words
  0–99 become digits (close to `qrtz-talk::bench::normalize`).
- `tailLostPct`: % of utterances whose reference **last word** is not among the hypothesis's last two words. A last
  word that was recognised wrongly also counts, so the floor is the model's own error on last words (~5 %). The
  Mac bench's `tailWordsLost` counts up to 3 missing words per clip instead.
- `finalMsP50/P90`: streaming: time from the end of feeding (every chunk that was ready already decoded) to the
  final text, i.e. pad + finish + decode, the release-side compute. Offline: the batch decode of the whole utterance.
  CPU, contended (the thread count is in the row's note).
- `rtf`: total compute / audio duration.
