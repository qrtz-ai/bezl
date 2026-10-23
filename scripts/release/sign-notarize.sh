#!/usr/bin/env bash
# Audit P0-1: one command for a Developer ID signed, notarized and stapled qrtz release on Jared's Mac.
#
#   bun run release:mac                                        # build, sign, notarize, staple, verify, sha256
#   scripts/release/sign-notarize.sh --app path/to/qrtz.app    # skip the build (a `bun run make` output)
#   scripts/release/sign-notarize.sh --no-notarize             # sign + verify only (nothing goes to Apple)
#
# Nothing secret lives in the repo or in this script. The defaults are Jared's public identifiers:
#   QRTZ_SIGN_ID          the Developer ID Application identity in the login keychain
#                         (`security find-identity -v -p codesigning`); APPLE_SIGNING_IDENTITY is read if it is unset.
#                         Default: Developer ID Application: Jared Grimes (MR26CCZ496).
#   QRTZ_TEAM_ID          its 10-character team id; every executable's signature is checked against it.
#                         Default: MR26CCZ496 (qrtz_modules::manifest::RELEASE_TEAM_ID, the team the bundled lock pins).
#   QRTZ_NOTARY_PROFILE   a notarytool keychain profile (default `qrtz-notary`), stored once with
#                         `xcrun notarytool store-credentials qrtz-notary --apple-id <apple id> --team-id MR26CCZ496`
#                         (it prompts for an app-specific password from account.apple.com; never paste it anywhere).
#
# Steps:
#   1. `bun run make` with APPLE_SIGNING_IDENTITY=$QRTZ_SIGN_ID and every notarization variable unset: Tauri signs the
#      app and its externalBins with the hardened runtime but doesn't notarize yet.
#   2. scripts/release/sign-mac.sh (ADR 0162): re-signs the nested executables inside out the way
#      docs/modules/confinement-map.json bundledCode says (the `qrtz` CLI and qrtz-snap with no entitlements,
#      qrtz-helper with the app's), reseals qrtz.app (qrtz-app is its main executable), checks DRs, runtime and
#      entitlements, then notarizes and staples the app and a fresh .dmg.
#   3. Checks here: every Mach-O in Contents/MacOS is signed by $QRTZ_TEAM_ID with the hardened runtime, the app
#      and dmg are stapled and pass Gatekeeper, then writes <dmg>.sha256.
#
# QRTZ_SIGN_DRY_RUN=1 prints every command instead of running it (works on Linux; the tests use it).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DRY="${QRTZ_SIGN_DRY_RUN:-0}"
PROFILE="${QRTZ_NOTARY_PROFILE:-qrtz-notary}"
APP=""
NOTARIZE=1

die() { echo "sign-notarize: $*" >&2; exit 1; }
run() {
  if [ "$DRY" = 1 ]; then
    printf '+'; printf ' %q' "$@"; printf '\n'
  else
    "$@"
  fi
}
# shellcheck source=scripts/release/stapler-retry.sh
source "$ROOT/scripts/release/stapler-retry.sh"
staple_validate() { # path (dry run prints the plain command; real runs retry network drops, QRT-354)
  if [ "$DRY" = 1 ]; then run xcrun stapler validate "$1"; else stapler_retry "$1" validate; fi
}

while [ $# -gt 0 ]; do
  case "$1" in
    --app) APP="${2:?--app needs a path}"; shift 2 ;;
    --no-notarize) NOTARIZE=0; shift ;;
    -h|--help) sed -n '2,31p' "$0"; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

RELEASE_SIGN_ID="Developer ID Application: Jared Grimes (MR26CCZ496)"
QRTZ_SIGN_ID="${QRTZ_SIGN_ID:-${APPLE_SIGNING_IDENTITY:-$RELEASE_SIGN_ID}}"
QRTZ_TEAM_ID="${QRTZ_TEAM_ID:-MR26CCZ496}"
[[ "$QRTZ_TEAM_ID" =~ ^[0-9A-Z]{10}$ ]] || die "QRTZ_TEAM_ID must be 10 characters, A-Z and 0-9"
[ "$QRTZ_TEAM_ID" != 0000000000 ] || die "QRTZ_TEAM_ID is the placeholder team"
case "$QRTZ_SIGN_ID" in
  "Developer ID Application:"*) ;;
  *) die "QRTZ_SIGN_ID must be a Developer ID Application identity (not Apple Development or ad-hoc)" ;;
esac
case "$QRTZ_SIGN_ID" in
  *"($QRTZ_TEAM_ID)") ;;
  *) die "QRTZ_SIGN_ID doesn't name team $QRTZ_TEAM_ID" ;;
esac

# The speech module source is baked in at build time (docs/release.md). The five QRTZ_SPEECH_* vars are all or none:
# none set builds without a speech module; only some set is a misconfigured release and stops before anything runs.
# Only names are printed, never values. Not checked with --app (that build already has its source baked in).
SPEECH_VARS=(QRTZ_SPEECH_URL QRTZ_SPEECH_KEY_ID QRTZ_SPEECH_KEY QRTZ_SPEECH_ROOT_KEY_ID QRTZ_SPEECH_ROOT_KEY)
if [ -z "$APP" ]; then
  speech_set=(); speech_missing=()
  for v in "${SPEECH_VARS[@]}"; do
    if [ -n "${!v:-}" ]; then speech_set+=("$v"); else speech_missing+=("$v"); fi
  done
  if [ "${#speech_set[@]}" -eq 0 ]; then
    echo "sign-notarize: no QRTZ_SPEECH_* set: building without a speech module" >&2
  elif [ "${#speech_missing[@]}" -gt 0 ]; then
    die "QRTZ_SPEECH_* are all or none; set: ${speech_set[*]}; missing: ${speech_missing[*]}"
  else
    echo "sign-notarize: speech module source: all ${#SPEECH_VARS[@]} QRTZ_SPEECH_* set" >&2
  fi
fi

if [ "$DRY" != 1 ]; then
  [ "$(uname -s)" = Darwin ] || die "signing needs macOS"
  for t in codesign xcrun spctl shasum bun; do command -v "$t" >/dev/null || die "missing $t"; done
  security find-identity -v -p codesigning | grep -Fq "\"$QRTZ_SIGN_ID\"" \
    || die "identity not in the keychain: $QRTZ_SIGN_ID"
  if [ "$NOTARIZE" = 1 ]; then
    # QRTZ_NOTARY_KEYCHAIN: CI keeps the profile in its temporary keychain (ADR 0179/0181), as notarize.sh reads it.
    NKC=()
    [ -n "${QRTZ_NOTARY_KEYCHAIN:-}" ] && NKC=(--keychain "$QRTZ_NOTARY_KEYCHAIN")
    xcrun notarytool history --keychain-profile "$PROFILE" "${NKC[@]+"${NKC[@]}"}" >/dev/null 2>&1 \
      || die "no notarytool profile '$PROFILE' (xcrun notarytool store-credentials $PROFILE …)"
  fi
fi

# 1. Build: signed by Tauri, not notarized (sign-mac.sh fixes the nested signatures first).
NO_NOTARY=(-u APPLE_ID -u APPLE_PASSWORD -u APPLE_TEAM_ID -u APPLE_API_KEY -u APPLE_API_ISSUER -u APPLE_API_KEY_PATH)
if [ -z "$APP" ]; then
  run env "${NO_NOTARY[@]}" APPLE_SIGNING_IDENTITY="$QRTZ_SIGN_ID" bun run --cwd "$ROOT/apps/desktop" make
  APP="${CARGO_TARGET_DIR:-$ROOT/target}/release/bundle/macos/qrtz.app"
  if [ "$DRY" != 1 ] && [ ! -d "$APP" ] && [ -d "$ROOT/apps/desktop/src-tauri/target/release/bundle/macos/qrtz.app" ]; then
    APP="$ROOT/apps/desktop/src-tauri/target/release/bundle/macos/qrtz.app"
  fi
fi
[ "$DRY" = 1 ] || [ -d "$APP/Contents/MacOS" ] || die "not an app bundle: $APP"

# 2. Nested signatures, reseal, checks, notarize + staple (app and dmg).
SIGN_ARGS=("$APP")
[ "$NOTARIZE" = 1 ] || SIGN_ARGS+=(--no-notarize)
run env "${NO_NOTARY[@]}" APPLE_SIGNING_IDENTITY="$QRTZ_SIGN_ID" QRTZ_NOTARY_PROFILE="$PROFILE" \
  bash "$ROOT/scripts/release/sign-mac.sh" "${SIGN_ARGS[@]}"

# 3. Every executable signed by this team with the hardened runtime.
check_team() { # file
  local info
  info="$(codesign -dv --verbose=2 "$1" 2>&1)" || die "$1 is not signed"
  grep -qx "TeamIdentifier=$QRTZ_TEAM_ID" <<<"$info" || die "$1 is not signed by team $QRTZ_TEAM_ID"
  grep -q 'flags=.*runtime' <<<"$info" || die "$1 lacks the hardened runtime"
}
if [ "$DRY" = 1 ]; then
  run codesign -dv --verbose=2 "$APP"
else
  check_team "$APP"
  for f in "$APP/Contents/MacOS/"*; do [ -f "$f" ] && check_team "$f"; done
fi

if [ "$NOTARIZE" = 1 ]; then
  VERSION="$( [ "$DRY" = 1 ] && echo '<version>' || /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
  DMG="$(dirname "$APP")/qrtz_${VERSION}_aarch64.dmg"
  staple_validate "$APP"
  staple_validate "$DMG"
  run spctl --assess --type execute --verbose=2 "$APP"
  run spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG"
  # The release gate (scripts/release/verify-bundle.sh): launch requirements, entitlements, notarization, staple.
  run bash "$ROOT/scripts/release/verify-bundle.sh" "$APP"
  if [ "$DRY" = 1 ]; then
    run shasum -a 256 "$DMG"
  else
    (cd "$(dirname "$DMG")" && shasum -a 256 "$(basename "$DMG")" >"$(basename "$DMG").sha256")
    GK="$(spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG" 2>&1 || true)"
    echo "signed, notarized and stapled: $DMG ($(cut -d' ' -f1 <"$DMG.sha256"))"
    echo "== release"
    echo "dmg:        $DMG"
    echo "sha256:     $(cut -d' ' -f1 <"$DMG.sha256")"
    echo "gatekeeper: $(tr '\n' ' ' <<<"$GK")"
  fi
else
  echo "signed (not notarized): $APP"
fi
