#!/usr/bin/env bash
# QRT-299: a release qrtz-helper (no QRTZ_DIAG) refuses --bench, --echo-bench and --echo-warmup as unknown options
# (exit 64) and carries none of those strings. Usage: scripts/release-flags-check.sh [helper binary]
# (default: builds `swift build -c release --product qrtz-helper`).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
bin="${1:-}"
if [[ -z "$bin" ]]; then
  # shellcheck disable=SC2086 # SWIFT_BUILD_FLAGS is whitespace-split on purpose, as build-helper.mjs does
  (cd "$here" && swift build -c release --product qrtz-helper ${SWIFT_BUILD_FLAGS:-})
  # shellcheck disable=SC2086
  bin="$(cd "$here" && swift build -c release --show-bin-path ${SWIFT_BUILD_FLAGS:-})/qrtz-helper"
fi
status=0
for flag in --bench --echo-bench --echo-warmup; do
  set +e
  err="$("$bin" "$flag" x 2>&1 >/dev/null </dev/null)"
  code=$?
  set -e
  if [[ $code -eq 64 && "$err" == *"unknown option $flag"* ]]; then
    echo "release-flags-check: ok: $flag refused"
  else
    echo "release-flags-check: FAIL: $flag not refused (exit $code): $err" >&2; status=1
  fi
  if LC_ALL=C grep -aqF -e "$flag" "$bin"; then
    echo "release-flags-check: FAIL: the release binary contains the string $flag" >&2; status=1
  fi
done
exit $status
