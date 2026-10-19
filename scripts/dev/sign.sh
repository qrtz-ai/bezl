#!/usr/bin/env bash
# Sign a dev build (target/debug) so macOS privacy grants (TCC) stick across rebuilds.
#
#   scripts/dev/sign.sh [<target>/debug]        # default: $CARGO_TARGET_DIR/debug or <repo>/target/debug
#
# The linker gives target/debug/qrtz-app an ad-hoc signature named qrtz_app-<hash>, whose designated requirement is
# its cdhash: every rebuild is a "new app" to TCC and Accessibility silently stops working. This re-signs qrtz-app as
# ai.qrtz.desktop.dev and its helpers with the identifiers the release uses (sign-mac.sh NESTED), with a real
# certificate, so the requirement is "this identifier + this certificate" and survives rebuilds. The app id is NOT
# ai.qrtz.desktop on purpose: that TCC row belongs to the Developer ID qrtz.app in /Applications, and two signers
# sharing one row would keep invalidating each other's grant.
#
# Identity: $QRTZ_DEV_SIGN_ID if set ("-" forces ad-hoc), else the first "Apple Development:" identity in the
# keychain, else ad-hoc. Ad-hoc keeps the stable identifier but TCC still pins the cdhash, so grants won't survive a
# rebuild; install an Apple Development certificate (Xcode → Settings → Accounts) to fix that. No hardened runtime
# and no entitlements: dev builds don't need them (the release signs with both, scripts/release/sign-mac.sh).
# Unchanged binaries already signed this way are left alone. QRTZ_DEV_NO_SIGN=1 skips everything.
set -euo pipefail

[[ "$(uname -s)" == Darwin ]] || exit 0
[[ "${QRTZ_DEV_NO_SIGN:-0}" == 1 ]] && exit 0
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
dir="${1:-${CARGO_TARGET_DIR:-$repo/target}/debug}"
log() { echo "[qrtz dev sign] $*" >&2; }

identity="${QRTZ_DEV_SIGN_ID:-}"
if [[ -z "$identity" ]]; then
  identity="$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p' | head -1)"
  [[ -n "$identity" ]] || identity="-"
fi

# codesign can wait on a keychain "allow access" prompt; never hang the dev loop on it.
with_timeout() { perl -e 'alarm shift; exec @ARGV or die' "$@"; }

signed_as() { # file id identity: already signed with this identifier by this identity, and valid
  local info
  info="$(codesign -dvv "$1" 2>&1)" || return 1
  grep -qx "Identifier=$2" <<<"$info" || return 1
  if [[ "$3" == "-" ]]; then grep -q '^Signature=adhoc' <<<"$info" || return 1
  else grep -qxF "Authority=$3" <<<"$info" || return 1; fi
  codesign --verify "$1" 2>/dev/null
}

sign() { # name id
  local bin="$dir/$1"
  [[ -f "$bin" ]] || return 0
  grep -q "qrtz: dev stub" "$bin" 2>/dev/null && return 0
  signed_as "$bin" "$2" "$identity" && return 0
  if with_timeout 60 codesign --force --identifier "$2" --sign "$identity" "$bin" 2>/dev/null; then
    log "$1: $2 ($identity)"
  elif [[ "$identity" != "-" ]] && codesign --force --identifier "$2" --sign - "$bin" 2>/dev/null; then
    log "$1: $2 (ad-hoc: signing with '$identity' failed or timed out; allow codesign in the keychain prompt)"
  else
    log "$1: signing failed"; return 1
  fi
}

# Inside out: helpers first, the app last. Must match scripts/release/sign-mac.sh NESTED.
sign qrtz-helper qrtz-helper
sign qrtz-snap ai.qrtz.snap
sign qrtz ai.qrtz.cli
sign qrtz-app ai.qrtz.desktop.dev
