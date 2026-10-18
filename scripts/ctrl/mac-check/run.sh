#!/bin/bash
# Runs on the Mac from the extracted bundle (scripts/ctrl/mac-check.sh). Everything stays under this directory: the
# SwiftPM cache / config / security paths and both scratch paths, so nothing lands in ~/Library.
D="$(cd "$(dirname "$0")" && pwd)"
# --disable-keychain: SwiftPM never queries the login keychain (run 24 stalled 21 min on a binary-artifact download
# with a SecurityAgent dialog already frontmost; ADR 0141).
SP=(--cache-path "$D/spm/cache" --config-path "$D/spm/config" --security-path "$D/spm/security" --disable-keychain)
killtree() { local c; for c in $(pgrep -P "$1"); do killtree "$c"; done; kill "$1" 2>/dev/null; }
# capped SECS LOG DIR swift-args...: run `xcrun swift` in DIR, stop it after SECS (macOS has no timeout(1)).
capped() {
  local secs=$1 log=$2 dir=$3 t=0
  shift 3
  (cd "$dir" && exec xcrun swift "$@") >"$log" 2>&1 &
  local p=$!
  while kill -0 "$p" 2>/dev/null; do
    if [ "$t" -ge "$secs" ]; then
      echo "timed out after ${secs}s; last log lines:"; tail -3 "$log"
      killtree "$p"; wait "$p" 2>/dev/null; return 124
    fi
    sleep 5; t=$((t + 5))
  done
  wait "$p"
}
echo "== sw_vers"; sw_vers
echo "== swift"; xcrun swift --version 2>&1; xcode-select -p
echo "== build ctrl-check"
capped 600 "$D/check-build.log" "$D/ctrl-check" build "${SP[@]}" --scratch-path "$D/b-check"
echo "exit $?"; grep -E "error:" "$D/check-build.log" | head -40; tail -3 "$D/check-build.log"
echo "== check"
if [ -x "$D/b-check/debug/ctrl-check" ]; then "$D/b-check/debug/ctrl-check"; echo "exit $?"; fi
echo "== build qrtz-helper"
# ADR 0142: FluidAudio and its NemoTextProcessing artifact come in the bundle (checksummed on the box), so this build
# fetches nothing. ditto keeps the xcframework layout.
if [ -f "$D/FluidAudio/NemoTextProcessing.xcframework.zip" ]; then
  ditto -x -k "$D/FluidAudio/NemoTextProcessing.xcframework.zip" "$D/FluidAudio" && rm -f "$D/FluidAudio/NemoTextProcessing.xcframework.zip"
  echo "artifact unpacked: $(ls -d "$D/FluidAudio/NemoTextProcessing.xcframework" 2>&1)"
fi
s=$(date +%s)
capped 900 "$D/helper-build.log" "$D/qrtz-helper" build --product qrtz-helper "${SP[@]}" --scratch-path "$D/b-helper"
echo "exit $? in $(( $(date +%s) - s ))s"
grep -E "error:" "$D/helper-build.log" | sort -u | head -60
echo "warnings in QrtzCtrl: $(grep -E 'Sources/QrtzCtrl/.*warning:' "$D/helper-build.log" | sort -u | wc -l)"
grep -E 'Sources/QrtzCtrl/.*warning:' "$D/helper-build.log" | sort -u | head -20
echo "warnings total: $(grep -E 'warning:' "$D/helper-build.log" | sort -u | wc -l)"
tail -3 "$D/helper-build.log"
echo "== QrtzCtrlTests"
# ADR 0141: the Mac check is read-only end to end. These CtrlMacGuardTests open TextEdit, press its close button,
# post key events (to the test process) or call AX actions on Finder, so they're skipped here unless
# QRTZ_CTRL_MAC_ACTIVE=1. The session probe and the temp-dir downloads cleanup still run.
SKIP=()
if [ "${QRTZ_CTRL_MAC_ACTIVE:-}" != "1" ]; then
  SKIP=(--skip 'CtrlMacGuardTests\.(testBackgroundTextEdit|testPrivateSourceKeyEvents|testMacProbe|testForbiddenAXActions)')
  echo "skipping the active CtrlMacGuardTests (QRTZ_CTRL_MAC_ACTIVE=1 runs them)"
fi
s=$(date +%s)
capped 900 "$D/test.log" "$D/qrtz-helper" test --filter QrtzCtrlTests "${SKIP[@]}" "${SP[@]}" --scratch-path "$D/b-helper"
echo "exit $? in $(( $(date +%s) - s ))s"
grep -E "error:|failed|Executed .* tests" "$D/test.log" | sort -u | head -30; tail -3 "$D/test.log"
