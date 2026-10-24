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

# QRTZ_DEV_SIGN_IDENTITY is accepted as an alias of QRTZ_DEV_SIGN_ID. Never a "Developer ID" identity: that one is
# for release builds only (scripts/release/sign-mac.sh), so it is refused here.
identity="${QRTZ_DEV_SIGN_ID:-${QRTZ_DEV_SIGN_IDENTITY:-}}"
if [[ "$identity" == "Developer ID"* ]]; then
  log "refusing '$identity' for a dev build (release-only identity); using the Apple Development identity instead"
  identity=""
fi
if [[ -z "$identity" ]]; then
  identity="$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p' | head -1)"
  if [[ -z "$identity" ]]; then
    identity="-"
    log "WARNING: no 'Apple Development:' signing identity in the keychain; signing ad-hoc. Privacy grants (Accessibility"
    log "         & co.) will NOT survive rebuilds. Fix: Xcode > Settings > Accounts > Manage Certificates > + Apple Development."
  fi
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

sign() { # name id  (name may be an absolute path)
  local bin="$dir/$1"; [[ "$1" == /* ]] && bin="$1"
  [[ -f "$bin" ]] || return 0
  # Dev stubs are tiny shell scripts (scripts/sidecars.mjs, build.rs). Only skip those: a real Mach-O such as qrtz-app
  # contains the "qrtz: dev stub" string itself (qrtz-helper-bridge DEV_STUB_MARK), so grepping the whole file would
  # skip it and leave it ad-hoc, which is why grants kept resetting.
  [[ "$(head -c 2 "$bin" 2>/dev/null)" == '#!' ]] && grep -q "qrtz: dev stub" "$bin" 2>/dev/null && return 0
  signed_as "$bin" "$2" "$identity" && return 0
  if with_timeout 60 codesign --force --identifier "$2" --sign "$identity" "$bin" 2>/dev/null; then
    log "$1: $2 ($identity)"
  elif [[ "$identity" != "-" ]] && codesign --force --identifier "$2" --sign - "$bin" 2>/dev/null; then
    log "$1: $2 (ad-hoc: signing with '$identity' failed or timed out; allow codesign in the keychain prompt)"
  else
    log "$1: signing failed"; return 1
  fi
}

# Inside out: nested code first, the app last. Must match scripts/release/sign-mac.sh NESTED. ADR 0169: no module
# code is bundled; a dev monolith (QRTZ_HELPER_BIN) signs as ai.qrtz.helper (confinement-map.json moduleCode
# devHelper), and the installed modules carry their own signatures (scripts/modules/build-packages.sh).
if [[ -n "${QRTZ_HELPER_BIN:-}" && -f "$QRTZ_HELPER_BIN" ]]; then
  dir="$(dirname "$QRTZ_HELPER_BIN")" sign "$(basename "$QRTZ_HELPER_BIN")" ai.qrtz.helper || true
fi
# dev-local (scripts/dev/dev-local.sh) runs the module helpers straight from native/qrtz-helper/.build/debug via
# QRTZ_<ID>_BIN. SwiftPM signs them ad-hoc as qrtz-<id>-<hash>, a new cdhash every build; give them the release module
# identifiers (scripts/modules/build-packages.sh) and the same certificate so their requirement stays fixed too.
for m in nano snap ctrl echo; do
  var="QRTZ_$(tr '[:lower:]' '[:upper:]' <<<"$m")_BIN"
  [[ -n "${!var:-}" && -f "${!var}" ]] && { sign "${!var}" "ai.qrtz.$m" || true; }
done
sign qrtz ai.qrtz.cli
# ADR 0174: echo is bundled (Contents/MacOS/qrtz-echo, target/debug/qrtz-echo in dev), signed as in a release.
sign qrtz-echo ai.qrtz.echo
sign qrtz-app ai.qrtz.desktop.dev
