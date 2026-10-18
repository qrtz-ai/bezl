#!/bin/bash
# Every row of docs/echo/dictation-tuning.md §6. Run from this directory with models/, data/ and venv/ next to it
# (README.md). Each run is ~10 min on 4 CPU threads; WER and last-word numbers don't depend on the thread count.
set -euo pipefail
P=${PYTHON:-venv/bin/python}
M5=models/sherpa-onnx-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25
M11=models/sherpa-onnx-nemotron-speech-streaming-en-0.6b-1120ms-int8-2026-04-25
M35=models/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11
MP=models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8
mkdir -p results
$P dsp.py && $P compare.py --selfcheck
[ -f evalset.json ] || $P evalset.py
$P run.py $M5 --name nem560-pad640
$P run.py $M5 --name nem560-pad0 --pad 0
$P run.py $M5 --name nem560-pad1120 --pad 1.12
$P run.py $M5 --name nem560-trim-pad640 --trim
$P run.py $M5 --name nem560-trim-pad1120 --trim --pad 1.12
$P run.py $M5 --name nem560-trim150-pad640 --trim --trimkeep 0.15
$P run.py $M5 --name nem560-trim-finishpad --trim --pad 0 --chunkpad 8960
$P run.py $M11 --name nem1120-pad640
$P run.py $M11 --name nem1120-pad1280 --pad 1.28
$P run.py $M35 --name nem35-560-pad640
$P run.py $MP --name parakeet-v2-offline --offline
$P run.py $MP --name parakeet-v2-offline-trim --offline --trim
# Cycle 33, tuning doc §7 (run; cycle 34 re-ran hf10-linear with this copy: 20/20 hypotheses identical).
$P run.py $M5 --name fa-keep90-pad640 --trim --keep 90 --chunkpad 8960
$P run.py $M5 --name fa-keep190-pad640 --trim --keep 190 --chunkpad 8960
$P run.py $M5 --name fa-keep90-pad1120 --trim --keep 90 --chunkpad 8960 --pad 1.12
$P run.py $M5 --name head-drop0 --headdrop 0
$P run.py $M5 --name head-drop150 --headdrop 150
$P run.py $M5 --name head-drop300 --headdrop 300
$P run.py $M5 --name hf10-linear --hf 10 --resampler linear
$P run.py $M5 --name hf10-poly --hf 10 --resampler poly
# Written in cycle 33, run in cycles 35-37 (tuning doc §9, ADR 0115): the input side. A 48 kHz mic through the helper's linear resampler vs a windowed sinc, clean, A 48 kHz mic through the helper's linear resampler vs a windowed sinc, clean,
# with white noise at 15 dB and with 8-20 kHz noise at 15 dB; a quiet mic (x0.1) raw, peak-normalised (oracle) and
# with the causal per-block normaliser; Nemotron 3.5 with the language forced to English.
$P run.py $M5 --name nem560-r48lin-pad640 --via48 linear
$P run.py $M5 --name nem560-r48sinc-pad640 --via48 sinc
$P run.py $M5 --name nem560-wn15-pad640 --noise 15
$P run.py $M5 --name nem560-r48lin-wn15-pad640 --via48 linear --noise 15
$P run.py $M5 --name nem560-r48sinc-wn15-pad640 --via48 sinc --noise 15
$P run.py $M5 --name nem560-r48lin-hf15-pad640 --via48 linear --noise 15 --band hf
$P run.py $M5 --name nem560-r48sinc-hf15-pad640 --via48 sinc --noise 15 --band hf
$P run.py $M5 --name nem560-q01-pad640 --scale 0.1
$P run.py $M5 --name nem560-q01-peak-pad640 --scale 0.1 --norm peak
$P run.py $M5 --name nem560-q01-agc-pad640 --scale 0.1 --norm agc
$P run.py $M35 --name nem35-560-en-pad640 --lang en
# Cycle 37 (ADR 0115): the same causal normaliser on a mic at normal level, clean and with 15 dB white noise.
$P run.py $M5 --name nem560-agc-pad640 --norm agc
$P run.py $M5 --name nem560-wn15-agc-pad640 --noise 15 --norm agc
# Paired comparisons (95 % bootstrap interval over utterances), e.g.:
$P compare.py results/nem560-r48lin-wn15-pad640.json results/nem560-r48sinc-wn15-pad640.json
$P compare.py results/nem560-q01-pad640.json results/nem560-q01-agc-pad640.json
$P compare.py results/nem560-r48lin-hf15-pad640.json results/nem560-r48sinc-hf15-pad640.json
$P compare.py results/nem560-pad640.json results/nem560-agc-pad640.json
$P compare.py results/nem560-wn15-pad640.json results/nem560-wn15-agc-pad640.json
