#!/usr/bin/env bash
# ADR 0162, ADR 0159: sign the nested executables of a built qrtz.app the way docs/modules/confinement-map.json
# (bundledCode) says, reseal the app, check every signature, then notarize and staple the app and a fresh .dmg.
#
# Why: `tauri build` re-signs every externalBin with the app's entitlements (allow-jit, audio-input, apple-events)
# and the file name as identifier. The `qrtz` command needs none and qrtz-echo (ADR 0174) needs its own (microphone only). So the bundle is built signed but NOT notarized, fixed up here, then notarized.
#
# Usage (on the Mac, after `bun run make` with APPLE_SIGNING_IDENTITY set and APPLE_ID/APPLE_PASSWORD unset; usually
# through `bun run release:mac`, scripts/release/sign-notarize.sh):
#   scripts/release/sign-mac.sh <path/to/qrtz.app> [--no-notarize]
# Env (all optional): APPLE_SIGNING_IDENTITY or QRTZ_SIGN_ID, default
#      "Developer ID Application: Jared Grimes (MR26CCZ496)"; QRTZ_TEAM_ID, default MR26CCZ496 (the identity must name
#      it: the bundled lock's designated requirements pin it); notarytool keychain profile QRTZ_NOTARY_PROFILE
#      (default qrtz-notary, docs/modules/production-setup.md A3).
set -euo pipefail
# shellcheck source=scripts/release/stapler-retry.sh
source "$(dirname "${BASH_SOURCE[0]}")/stapler-retry.sh"

repo="$(cd "$(dirname "$0")/../.." && pwd)"
app="${1:?usage: sign-mac.sh <qrtz.app> [--no-notarize]}"
notarize=1
[[ "${2:-}" == "--no-notarize" ]] && notarize=0
identity="${APPLE_SIGNING_IDENTITY:-${QRTZ_SIGN_ID:-Developer ID Application: Jared Grimes (MR26CCZ496)}}"
profile="${QRTZ_NOTARY_PROFILE:-qrtz-notary}"
want_team="${QRTZ_TEAM_ID:-MR26CCZ496}"
team="$(sed -n 's/.*(\([A-Z0-9]\{10\}\)).*/\1/p' <<<"$identity")"
[[ -n "$team" ]] || { echo "can't read the team id from the signing identity ($identity)" >&2; exit 1; }
[[ "$team" == "$want_team" ]] || { echo "signing identity names team $team, want $want_team (QRTZ_TEAM_ID)" >&2; exit 1; }
app_ent="$repo/apps/desktop/src-tauri/entitlements.mac.plist"
macos="$app/Contents/MacOS"
[[ -x "$macos/qrtz-app" ]] || { echo "$macos/qrtz-app missing: is $app a qrtz.app built from main?" >&2; exit 1; }

# name identifier entitlements(app | none | repo path). Must match confinement-map.json bundledCode (checked by
# crates/qrtz-lifecycle/tests/offline_graph.rs).
# ADR 0174: the `qrtz` command. QRT-255: the bundled module helpers (echo's qrtz-echo today) are NOT listed here:
# their name, identifier, requirement and entitlements come from each modules/*/module.json (bundled-helpers.sh), the
# same file the launcher's requirement comes from. nano, ctrl and snap are signed in their packages
# (scripts/modules/build-packages.sh, moduleCode). qrtz-dash-net is never bundled or signed here (ADR 0231).
NESTED="
qrtz ai.qrtz.cli none
"
# shellcheck source=bundled-helpers.sh
source "$repo/scripts/release/bundled-helpers.sh"
helpers="$(bundled_helpers "$repo/modules" "$team")" || exit 1  # also refuses a requirement that doesn't pin $team
# name identifier entitlements requirement, tab-separated: the NESTED rows get the default Developer ID requirement.
default_dr() { echo "identifier \"$1\" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate leaf[subject.OU] = \"$team\""; }
SIGN=""
while read -r name id ent; do
  [[ -n "$name" ]] && SIGN+="$name"$'\t'"$id"$'\t'"$ent"$'\t'"$(default_dr "$id")"$'\n'
done <<<"$NESTED"
while IFS=$'\t' read -r name id ent req; do
  [[ -n "$name" ]] || continue
  SIGN+="$name"$'\t'"$id"$'\t'"$ent"$'\t'"$req"$'\n'
done <<<"$helpers"

ent_file() { case "$1" in app) echo "$app_ent" ;; none) echo "" ;; *) echo "$repo/$1" ;; esac; }
keys() { grep -o '<key>[^<]*</key>' | sed 's/<[^>]*>//g' | sort; }

# A dev stub is the tiny shell script apps/desktop/src-tauri/build.rs writes ("#!/bin/sh" + the marker). Only that is
# refused: a real Mach-O such as qrtz-app contains the "qrtz: dev stub" string itself (qrtz-helper-bridge DEV_STUB_MARK),
# so grepping the whole file blocked real release binaries. The check always runs; it is never skipped.
is_dev_stub() { [[ "$(head -c 2 "$1" 2>/dev/null)" == '#!' ]] && grep -q "qrtz: dev stub" "$1"; }

echo "== sign nested code"
while IFS=$'\t' read -r name id ent req; do
  [[ -n "$name" ]] || continue
  bin="$macos/$name"
  [[ -f "$bin" ]] || { echo "$bin missing (externalBin in tauri.macos.conf.json?)" >&2; exit 1; }
  if is_dev_stub "$bin"; then echo "$bin is a dev stub" >&2; exit 1; fi
  f="$(ent_file "$ent")"
  args=(--force --options runtime --timestamp --identifier "$id" --sign "$identity")
  [[ -n "$f" ]] && args+=(--entitlements "$f")
  codesign "${args[@]}" "$bin"
  echo "  $name: $id, entitlements ${ent}"
done <<<"$SIGN"

echo "== reseal qrtz.app"
codesign --force --options runtime --timestamp --entitlements "$app_ent" --sign "$identity" "$app"

echo "== check"
codesign --verify --strict --deep --verbose=2 "$app"
while IFS=$'\t' read -r name id ent req; do
  [[ -n "$name" ]] || continue
  bin="$macos/$name"
  why="$(check_helper "$bin" "$id" "$req")" || { echo "$name: $why" >&2; exit 1; }
  info="$(codesign -dv "$bin" 2>&1)" # not piped into grep -q: its early exit SIGPIPEs codesign under pipefail
  grep -q 'flags=0x10000(runtime)' <<<"$info" || { echo "$name: no hardened runtime" >&2; exit 1; }
  got="$(codesign -d --entitlements - --xml "$bin" 2>/dev/null | keys || true)"
  f="$(ent_file "$ent")"
  want=""
  [[ -n "$f" ]] && want="$(keys <"$f" || true)"
  if [[ "$got" != "$want" ]]; then
    echo "$name: entitlements are [$(tr '\n' ' ' <<<"$got")], want [$(tr '\n' ' ' <<<"$want")]" >&2
    exit 1
  fi
  echo "  $name: ok ($id, runtime, entitlements [$(tr '\n' ' ' <<<"$want")])"
done <<<"$SIGN"

[[ "$notarize" == 1 ]] || { echo "signed; not notarized (--no-notarize)"; exit 0; }

echo "== notarize and staple the app"
# notarize.sh fails unless Apple answers Accepted: `notarytool submit --wait` can exit 0 on a rejected upload.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
ditto -c -k --keepParent --sequesterRsrc "$app" "$work/qrtz.zip"
bash "$repo/scripts/release/notarize.sh" "$work/qrtz.zip" "$profile"
stapler_retry "$app" staple validate
spctl --assess --type execute --verbose=2 "$app"

echo "== the .dmg"
version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")"
dmg="$(dirname "$app")/qrtz_${version}_aarch64.dmg"
rm -f "$dmg"
mkdir "$work/dmg" && ditto "$app" "$work/dmg/qrtz.app" && ln -s /Applications "$work/dmg/Applications"
hdiutil create -volname qrtz -srcfolder "$work/dmg" -ov -format UDZO "$dmg" >/dev/null
codesign --force --timestamp --sign "$identity" "$dmg"
bash "$repo/scripts/release/notarize.sh" "$dmg" "$profile"
stapler_retry "$dmg" staple validate
stapler_retry "$app" validate && stapler_retry "$dmg" validate
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
"$macos/qrtz" --help >/dev/null && echo "  qrtz --help: ok"
echo "done: $app and $dmg"
