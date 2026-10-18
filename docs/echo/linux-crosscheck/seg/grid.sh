#!/bin/bash
# ADR 0118 grid (tuning §11). Run from ~/echo-linux after `venv/bin/python seg/seg_eval.py build`.
set -e
P="venv/bin/python seg/seg_eval.py run"
$P --name whole --whole --bsilence 300 --bcap 15
for s in 200 300 500; do $P --name s$s-c15-o0 --silence $s --cap 15 --overlap 0; done
$P --name s300-c15-o0-min0 --silence 300 --cap 15 --overlap 0 --minseg 0
for c in 10 20 6; do $P --name s300-c$c-o0 --silence 300 --cap $c --overlap 0; done
for o in 0.5 1; do
  $P --name s300-c15-o$o --silence 300 --cap 15 --overlap $o
  $P --name s300-c15-o${o}cap --silence 300 --cap 15 --overlap $o --ovmode cap
  $P --name s300-c6-o${o}cap --silence 300 --cap 6 --overlap $o --ovmode cap
done
$P --name s200-c15-o0.5 --silence 200 --cap 15 --overlap 0.5
$P --name s500-c15-o0.5 --silence 500 --cap 15 --overlap 0.5
$P --name s300-c10-o0.5 --silence 300 --cap 10 --overlap 0.5
$P --name s300-c20-o0.5 --silence 300 --cap 20 --overlap 0.5
$P --name s300-c10-o0.5cap --silence 300 --cap 10 --overlap 0.5 --ovmode cap
# Overlap with a re-decode of a blank segment from its cut (decodes are cached, so these cost nothing more).
$P --name s300-c15-o0.5r --silence 300 --cap 15 --overlap 0.5 --retry
$P --name s300-c15-o1r --silence 300 --cap 15 --overlap 1 --retry
$P --name s300-c8-o0 --silence 300 --cap 8 --overlap 0
$P --name s300-p200a6-c15-o0 --silence 300 --pause2 200 --after 6 --cap 15 --overlap 0
$P --name s300-p150a6-c15-o0 --silence 300 --pause2 150 --after 6 --cap 15 --overlap 0
$P --name s300-p200a4-c10-o0 --silence 300 --pause2 200 --after 4 --cap 10 --overlap 0
venv/bin/python seg/retime.py   # release CPU re-timed back to back (run on a quiet box)
venv/bin/python seg/table.py
