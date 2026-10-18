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
