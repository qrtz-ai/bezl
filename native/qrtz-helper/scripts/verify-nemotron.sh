#!/bin/bash
# ADR 0079 verification on an Apple Silicon Mac, in a scratch worktree (your checkout is not touched):
#   swift build + swift test, the pinned models downloaded and sha256-checked (crates/qrtz-app/src/local_models.json),
#   then `qrtz-helper --bench`: two `say` voices on the mic and one on the system channel -> text, speaker turns,
#   talk latency (first partial, release to final) and memory. Prints one JSON report at the end.
# Usage: bash native/qrtz-helper/scripts/verify-nemotron.sh [repo=~/code/qrtz]   (KEEP=1 keeps /tmp/qrtz-verify*)
set -euo pipefail
REPO=${1:-$HOME/code/qrtz}
WT=/tmp/qrtz-verify
MODELS=/tmp/qrtz-verify-models
CLIPS=/tmp/qrtz-verify-clips
cleanup() {
  [ "${KEEP:-0}" = 1 ] && return
  git -C "$REPO" worktree remove --force "$WT" 2>/dev/null || rm -rf "$WT"
  rm -rf "$MODELS" "$CLIPS"
}
trap cleanup EXIT

echo "== worktree ($WT at origin/main)"
git -C "$REPO" fetch -q origin
git -C "$REPO" worktree remove --force "$WT" 2>/dev/null || rm -rf "$WT"
git -C "$REPO" worktree add -q --detach "$WT" origin/main
git -C "$WT" log --oneline -1

echo "== swift test"
cd "$WT/native/qrtz-helper"
swift test 2>&1 | grep -E "error:|Executed [0-9]+ tests" | tail -3
echo "== swift build -c release"
swift build -c release 2>&1 | tail -1
BIN="$(swift build -c release --show-bin-path)/qrtz-helper"

echo "== models (pinned, ~845 MB)"
python3 - "$WT/crates/qrtz-app/src/local_models.json" "$MODELS" <<'PY'
import hashlib, json, os, subprocess, sys
m = json.load(open(sys.argv[1])); root = sys.argv[2]
for s in m["sets"]:
    d = os.path.join(root, s["dir"]); os.makedirs(d, exist_ok=True)
    for f in s["files"]:
        p = os.path.join(d, f["path"]); os.makedirs(os.path.dirname(p), exist_ok=True)
        url = f'{m["base"]}/{s["repo"]}/resolve/{s["revision"]}/{f["remote"]}'
        if not (os.path.exists(p) and os.path.getsize(p) == f["size"]):
            subprocess.run(["curl", "-sfL", "--retry", "3", "-C", "-", "-o", p, url], check=True)
        h = hashlib.sha256()
        with open(p, "rb") as fh:
            for b in iter(lambda: fh.read(1 << 20), b""): h.update(b)
        assert h.hexdigest() == f["sha256"] and os.path.getsize(p) == f["size"], f"{p}: checksum mismatch"
    open(os.path.join(d, ".qrtz-verified"), "w").write(m["version"])
    print(f'{s["id"]}: {len(s["files"])} files verified')
PY

echo "== clips (say)"
mkdir -p "$CLIPS"
say -v Samantha -o "$CLIPS/a.aiff" "Morning everyone. I pushed the fix for the login bug last night, and the tests are green now."
say -v Daniel -o "$CLIPS/b.aiff" "Thanks. Can we also look at the export timing before Friday? It still takes about ten seconds."
say -v Karen -o "$CLIPS/c.aiff" "Sure. I will profile the export today and send the numbers to the channel this afternoon."
# Echo: the bench trims trailing silence, so the last word ("pineapple") ends on the clip's last sample.
say -v Samantha -o "$CLIPS/d.aiff" "Please move the launch review to Thursday and add a note about pineapple"

echo "== bench"
QRTZ_MODELS_DIR="$MODELS" "$BIN" --bench --mic "$CLIPS/a.aiff,$CLIPS/b.aiff" --system "$CLIPS/c.aiff" --dictation "$CLIPS/d.aiff" \
  | tee /tmp/qrtz-verify-report.json
echo "report: /tmp/qrtz-verify-report.json"
