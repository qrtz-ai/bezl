#!/usr/bin/env bash
# QRT-270: the echo warm-up race on a macOS runner (no mic, no Accessibility, no key press). Builds qrtz-helper,
# fetches the pinned Parakeet TDT v2 set (fetch-echo-model.py), speaks a clip with `say`, then runs three cold
# launches of `qrtz-helper --echo-warmup`, one push-to-talk press each: at launch (t≈0), 1 s in (model still
# loading) and right after `echo asr: ready`. Each launch loads the model from a fresh path, so Core ML specializes
# it again (the first-launch case Jared hit). Fails when any press gives empty or wrong text or a press before ready
# isn't held by the gate. Usage: scripts/echo-warmup-ci.sh [work dir] (default $RUNNER_TEMP or /tmp).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
work="${1:-${RUNNER_TEMP:-/tmp}/echo-warmup}"
models="${QRTZ_WARMUP_MODELS:-$work/models}"
mkdir -p "$work" "$models"
phrase="${QRTZ_WARMUP_PHRASE:-The quick brown fox jumps over the lazy dog.}"

python3 "$here/scripts/fetch-echo-model.py" "$models"
say -o "$work/clip.wav" --data-format=LEF32@16000 "$phrase"
(cd "$here" && swift build -c release --product qrtz-helper)
bin="$(cd "$here" && swift build -c release --show-bin-path)/qrtz-helper"

status=0
for at in 0 1000 ready; do
  run="$work/run-$at"; rm -rf "$run"; mkdir -p "$run/models"
  cp -R "$models/parakeet-tdt-v2" "$run/models/"   # a new path: no Core ML specialization cached for it
  echo "::group::press at $at"
  if ! QRTZ_MODELS_DIR="$run/models" "$bin" --echo-warmup "$work/clip.wav" --expect "$phrase" --press-at "$at" \
      --out "$run/result.json" 2> >(tee "$run/stderr.log" >&2); then status=1; fi
  echo "::endgroup::"
done

python3 - "$work" <<'PY'
import json, os, re, sys
work = sys.argv[1]
rows = ["| press | model load | warm-up decode | ready | press | held (still warming) | release→text | press→text | WER | text | result |",
        "|---|---|---|---|---|---|---|---|---|---|---|"]
for at in ("0", "1000", "ready"):
    d = os.path.join(work, f"run-{at}")
    try: r = json.load(open(os.path.join(d, "result.json")))
    except Exception: rows.append(f"| {at} | | | | | | | | | | FAIL (no result) |"); continue
    err = open(os.path.join(d, "stderr.log")).read()
    load = re.search(r"parakeet: loaded in (\d+) ms", err); warm = re.search(r"warm-up decode (\d+) ms", err)
    ms = lambda v: "" if v is None else f"{int(v)} ms"
    rows.append(f"| {at} | {load.group(1) + ' ms' if load else ''} | {warm.group(1) + ' ms' if warm else ''} | {ms(r['readyMs'])} | "
                f"{ms(r['pressMs'])} | {'yes' if r['stillWarming'] else 'no (ready)' if r['readyBeforePress'] else 'NO'} | "
                f"{ms(r['releaseToTextMs'])} | {ms(r['pressToTextMs'])} | {r['wer']:.2f} | {r['text']} | "
                f"{'pass' if r['ok'] else 'FAIL: ' + '; '.join(r['failures'])} |")
table = "\n".join(rows)
print(table)
s = os.environ.get("GITHUB_STEP_SUMMARY")
if s: open(s, "a").write("### echo warm-up (QRT-270)\n\nTimes are ms since launch.\n\n" + table + "\n")
PY
exit $status
