# shellcheck shell=bash
# QRT-354: retry `xcrun stapler` on Apple network drops. Apple's ticket lookup (api.apple-cloudkit.com) sometimes
# drops the connection (exit 68, NSURLError -1005), which failed app-v0.3.3 twice after notarization was accepted.
# On a network error the whole sequence (e.g. staple then validate) reruns at once, until it succeeds. There is no
# attempt cap or sleep here: the bound is the GitHub step's timeout-minutes in .github/workflows/release.yml.
# Any other failure returns immediately with stapler's exit code.
# Usage (sourced): stapler_retry <path> <action>...   e.g. stapler_retry "$dmg" staple validate
# QRTZ_STAPLER_BIN exists for tests only.

stapler_is_network_error() { # exit-code output
  [ "$1" = 68 ] && return 0
  grep -qiE -- '-1005|-1001|network connection was lost|timed out' <<<"$2"
}

stapler_retry() { # path action...
  local target="$1" attempt=0 action rc out
  shift
  local -a stapler_cmd
  read -r -a stapler_cmd <<<"${QRTZ_STAPLER_BIN:-xcrun stapler}"
  while :; do
    attempt=$((attempt + 1))
    rc=0
    for action in "$@"; do
      echo "stapler $action: attempt $attempt: $target" >&2
      out="$("${stapler_cmd[@]}" "$action" "$target" 2>&1)" || rc=$?
      [ -n "$out" ] && printf '%s\n' "$out"
      [ "$rc" = 0 ] || break
    done
    if [ "$rc" = 0 ]; then
      echo "stapler $*: ok on attempt $attempt: $target" >&2
      return 0
    fi
    if ! stapler_is_network_error "$rc" "$out"; then
      echo "stapler $action: failed with exit $rc (not a network error); not retrying" >&2
      return "$rc"
    fi
    echo "stapler $action: network error (exit $rc) on attempt $attempt; rerunning $*" >&2
  done
}
