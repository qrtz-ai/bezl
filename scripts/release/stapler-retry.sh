# shellcheck shell=bash
# QRT-354: retry `xcrun stapler` on Apple network drops. Apple's ticket lookup (api.apple-cloudkit.com) sometimes
# drops the connection (exit 68, NSURLError -1005), which failed app-v0.3.3 twice after notarization was accepted.
# On a network error (nonzero exit with -1005, -1001, 'timed out' or 'network connection was lost' in the output) the
# whole sequence (e.g. staple then validate) reruns with exponential backoff; any other failure returns at once.
#   QRTZ_STAPLE_INITIAL_DELAY  seconds before the 2nd attempt, doubling each time (default 15)
#   QRTZ_STAPLE_MAX_DELAY      cap on one wait, seconds (default 120)
#   QRTZ_STAPLE_MAX_ATTEMPTS   total attempts (default 8), then fail loudly
# Each must be a positive integer (empty, non-numeric or 0 is rejected); delays may be 0 only with QRTZ_STAPLER_TEST=1.
# The release step's timeout-minutes in .github/workflows/release.yml is the outer backstop.
# Usage (sourced): stapler_retry <path> <action>...   e.g. stapler_retry "$dmg" staple validate
# Release runs always use xcrun stapler. QRTZ_STAPLER_BIN is honored only with QRTZ_STAPLER_TEST=1 (stapler-retry.test.sh).

stapler_is_network_error() { # output (exit 68 alone is not enough: only a real network error in the output counts)
  grep -qiE -- '-1005|-1001|network connection was lost|timed out' <<<"$1"
}

stapler_int() { # name value min -> echoes value, or fails if not an integer >= min
  if ! [[ "$2" =~ ^[0-9]+$ ]] || [ "$((10#$2))" -lt "$3" ]; then
    echo "stapler_retry: $1 must be an integer >= $3, got '$2'" >&2
    return 2
  fi
  echo "$((10#$2))"
}

stapler_retry() { # path action...
  local target="$1" attempt=0 action rc out delay max_delay max_attempts min_delay=1
  shift
  [ "${QRTZ_STAPLER_TEST:-0}" = 1 ] && min_delay=0
  delay="$(stapler_int QRTZ_STAPLE_INITIAL_DELAY "${QRTZ_STAPLE_INITIAL_DELAY-15}" "$min_delay")" || return 2
  max_delay="$(stapler_int QRTZ_STAPLE_MAX_DELAY "${QRTZ_STAPLE_MAX_DELAY-120}" "$min_delay")" || return 2
  max_attempts="$(stapler_int QRTZ_STAPLE_MAX_ATTEMPTS "${QRTZ_STAPLE_MAX_ATTEMPTS-8}" 1)" || return 2
  local -a stapler_cmd
  stapler_cmd=(xcrun stapler)
  if [ "${QRTZ_STAPLER_TEST:-0}" = 1 ] && [ -n "${QRTZ_STAPLER_BIN:-}" ]; then stapler_cmd=("$QRTZ_STAPLER_BIN"); fi
  while :; do
    attempt=$((attempt + 1))
    rc=0
    for action in "$@"; do
      echo "stapler $action: attempt $attempt/$max_attempts: $target" >&2
      out="$("${stapler_cmd[@]}" "$action" "$target" 2>&1)" || rc=$?
      [ -n "$out" ] && printf '%s\n' "$out"
      [ "$rc" = 0 ] || break
    done
    if [ "$rc" = 0 ]; then
      echo "stapler $*: ok on attempt $attempt/$max_attempts: $target" >&2
      return 0
    fi
    if ! stapler_is_network_error "$out"; then
      echo "stapler $action: failed with exit $rc (not a network error); not retrying" >&2
      return "$rc"
    fi
    if [ "$attempt" -ge "$max_attempts" ]; then
      echo "::error::stapler $action $target: network error (exit $rc) on all $max_attempts attempts; giving up (QRT-354)" >&2
      return "$rc"
    fi
    echo "stapler $action: network error (exit $rc) on attempt $attempt/$max_attempts; rerunning $* in ${delay}s" >&2
    sleep "$delay"
    delay=$((delay * 2 > max_delay ? max_delay : delay * 2))
  done
}
