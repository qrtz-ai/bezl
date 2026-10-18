#!/bin/bash
# Runs on the Mac from the extracted bundle (scripts/ctrl/mac-check.sh). Everything stays under this directory: the
# SwiftPM cache / config / security paths and both scratch paths, so nothing lands in ~/Library.
D="$(cd "$(dirname "$0")" && pwd)"
SP=(--cache-path "$D/spm/cache" --config-path "$D/spm/config" --security-path "$D/spm/security")
echo "== sw_vers"; sw_vers
echo "== swift"; xcrun swift --version 2>&1; xcode-select -p
echo "== build ctrl-check"
(cd "$D/ctrl-check" && xcrun swift build "${SP[@]}" --scratch-path "$D/b-check" >"$D/check-build.log" 2>&1)
echo "exit $?"; grep -E "error:" "$D/check-build.log" | head -40; tail -3 "$D/check-build.log"
echo "== check"
if [ -x "$D/b-check/debug/ctrl-check" ]; then "$D/b-check/debug/ctrl-check"; echo "exit $?"; fi
echo "== build qrtz-helper"
s=$(date +%s)
(cd "$D/qrtz-helper" && xcrun swift build --product qrtz-helper "${SP[@]}" --scratch-path "$D/b-helper" >"$D/helper-build.log" 2>&1)
echo "exit $? in $(( $(date +%s) - s ))s"
grep -E "error:" "$D/helper-build.log" | sort -u | head -60
echo "warnings in QrtzCtrl: $(grep -E 'Sources/QrtzCtrl/.*warning:' "$D/helper-build.log" | sort -u | wc -l)"
grep -E 'Sources/QrtzCtrl/.*warning:' "$D/helper-build.log" | sort -u | head -20
echo "warnings total: $(grep -E 'warning:' "$D/helper-build.log" | sort -u | wc -l)"
tail -3 "$D/helper-build.log"
echo "== QrtzCtrlTests"
s=$(date +%s)
(cd "$D/qrtz-helper" && xcrun swift test --filter QrtzCtrlTests "${SP[@]}" --scratch-path "$D/b-helper" >"$D/test.log" 2>&1)
echo "exit $? in $(( $(date +%s) - s ))s"
grep -E "error:|failed|Executed .* tests" "$D/test.log" | sort -u | head -30; tail -3 "$D/test.log"
