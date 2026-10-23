#!/usr/bin/env bash
# QRT-354: stapler_retry against a fake stapler (any OS, no Apple access). Each case is one line in CASES:
#   name | fake script (one line per call: "<exit> <output>"; the last line repeats) | args | want exit | want calls
# A give-up-after-limit case can be added here once Jared picks the bound.
# Run by stapler-retry.test.mjs (vitest).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=scripts/release/stapler-retry.sh
source "$here/stapler-retry.sh"
t="$(mktemp -d)"; trap 'rm -rf "$t"' EXIT
export QRTZ_STAPLER_TEST=1 QRTZ_STAPLER_BIN="$t/stapler"
cat >"$QRTZ_STAPLER_BIN" <<'X'
#!/usr/bin/env bash
n=$(( $(cat "$FAKE_DIR/calls") + 1 )); echo "$n" >"$FAKE_DIR/calls"
echo "$1" >>"$FAKE_DIR/log"
line="$(sed -n "${n}p" "$FAKE_DIR/script")"; [ -n "$line" ] || line="$(tail -n1 "$FAKE_DIR/script")"
echo "${line#* }"; exit "${line%% *}"
X
chmod +x "$QRTZ_STAPLER_BIN"
export FAKE_DIR="$t"

NET='68 Error Domain=NSURLErrorDomain Code=-1005 "The network connection was lost."'
pass=0; bad=0
check() { # name want_rc want_calls args... ; script already in $t/script
  local name="$1" want_rc="$2" want_calls="$3" rc=0; shift 3
  echo 0 >"$t/calls"; : >"$t/log"
  stapler_retry x.dmg "$@" >"$t/out" 2>&1 || rc=$?
  local calls; calls="$(cat "$t/calls")"
  if [ "$rc" = "$want_rc" ] && [ "$calls" = "$want_calls" ]; then echo "ok: $name"; pass=$((pass + 1))
  else echo "FAIL: $name: exit $rc (want $want_rc), calls $calls (want $want_calls)"; sed 's/^/  /' "$t/out"; bad=$((bad + 1)); fi
}

printf '%s\n%s\n%s\n' "$NET" "$NET" '0 The staple and validate action worked!' >"$t/script"
check "network drop twice, then staple+validate succeed" 0 4 staple validate

printf '%s\n' '68 Could not find base64 encoded ticket in response' >"$t/script"
check "exit 68 without a network error fails at once" 68 1 staple validate

printf '%s\n' '65 CloudKit query for x.dmg failed: Record not found' >"$t/script"
check "other failure fails at once" 65 1 staple

printf '%s\n%s\n' '1 The request timed out.' '0 ok' >"$t/script"
check "timed out (exit 1) retries" 0 2 validate

# Without QRTZ_STAPLER_TEST=1 the override is ignored and xcrun stapler runs (a fake xcrun on PATH records it).
mkdir "$t/bin"; printf '#!/usr/bin/env bash\necho "$*" >"%s/xcrun.log"\n' "$t" >"$t/bin/xcrun"; chmod +x "$t/bin/xcrun"
printf '%s\n' '0 ok' >"$t/script"; echo 0 >"$t/calls"
if (unset QRTZ_STAPLER_TEST; PATH="$t/bin:$PATH" stapler_retry x.dmg validate >/dev/null 2>&1) \
  && [ "$(cat "$t/xcrun.log" 2>/dev/null)" = "stapler validate x.dmg" ] && [ "$(cat "$t/calls")" = 0 ]; then
  echo "ok: without the test flag, xcrun stapler runs"; pass=$((pass + 1))
else echo "FAIL: without the test flag, QRTZ_STAPLER_BIN was used"; bad=$((bad + 1)); fi

echo "$pass passed, $bad failed"
[ "$bad" = 0 ]
