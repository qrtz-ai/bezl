# shellcheck shell=bash
# QRT-354: bounded retry around `xcrun stapler staple|validate`. Apple's ticket lookup (api.apple-cloudkit.com)
# sometimes drops the connection (exit 68, NSURLError -1005), which failed app-v0.3.3 twice after notarization was
# accepted. Retries only network failures: max 3 attempts, retried immediately (no waits). Any other failure returns at once.
# Usage (sourced): stapler_retry staple|validate <path>
# QRTZ_STAPLER_BIN exists for tests only.

stapler_is_network_error() { # exit-code output
  [ "$1" = 68 ] && return 0
  grep -qiE -- '-1005|-1001|network connection was lost|timed out' <<<"$2"
}

stapler_retry() { # staple|validate <path>
  local action="$1" target="$2" attempt=1 max=3 rc out
  local -a stapler_cmd
  read -r -a stapler_cmd <<<"${QRTZ_STAPLER_BIN:-xcrun stapler}"
  while :; do
    echo "stapler $action: attempt $attempt/$max: $target" >&2
    rc=0
    out="$("${stapler_cmd[@]}" "$action" "$target" 2>&1)" || rc=$?
    [ -n "$out" ] && printf '%s\n' "$out"
    if [ "$rc" = 0 ]; then
      echo "stapler $action: ok on attempt $attempt/$max" >&2
      return 0
    fi
    if ! stapler_is_network_error "$rc" "$out"; then
      echo "stapler $action: failed with exit $rc (not a network error); not retrying" >&2
      return "$rc"
    fi
    if [ "$attempt" -ge "$max" ]; then
      echo "stapler $action: network error (exit $rc) on attempt $attempt/$max; giving up" >&2
      return "$rc"
    fi
    echo "stapler $action: network error (exit $rc) on attempt $attempt/$max; retrying now" >&2
    attempt=$((attempt + 1))
  done
}
