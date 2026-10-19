#!/usr/bin/env bash
# ADR 0162, ADR 0159: sign the nested executables of a built qrtz.app the way docs/modules/confinement-map.json
# (bundledCode) says, reseal the app, check every signature, then notarize and staple the app and a fresh .dmg.
#
# Why: `tauri build` re-signs every externalBin with the app's entitlements (allow-jit, audio-input, apple-events)
# and the file name as identifier. qrtz-snap must carry no entitlement and sign as ai.qrtz.snap (its lock entry), and
# the `qrtz` command needs none. So the bundle is built signed but NOT notarized, fixed up here, then notarized.
#
# Usage (on the Mac, after `bun run make` with APPLE_SIGNING_IDENTITY set and APPLE_ID/APPLE_PASSWORD unset):
#   scripts/release/sign-mac.sh <path/to/qrtz.app> [--no-notarize]
# Env: APPLE_SIGNING_IDENTITY="Developer ID Application: <Name> (<TEAMID>)"; notarytool keychain profile
#      QRTZ_NOTARY_PROFILE (default qrtz-notary, docs/modules/production-setup.md A3).
set -euo pipefail

repo="$(cd "$(dirname "$0")/../.." && pwd)"
app="${1:?usage: sign-mac.sh <qrtz.app> [--no-notarize]}"
notarize=1
[[ "${2:-}" == "--no-notarize" ]] && notarize=0
identity="${APPLE_SIGNING_IDENTITY:?set APPLE_SIGNING_IDENTITY (security find-identity -v -p codesigning)}"
profile="${QRTZ_NOTARY_PROFILE:-qrtz-notary}"
team="$(sed -n 's/.*(\([A-Z0-9]\{10\}\)).*/\1/p' <<<"$identity")"
[[ -n "$team" ]] || { echo "can't read the team id from APPLE_SIGNING_IDENTITY ($identity)" >&2; exit 1; }
app_ent="$repo/apps/desktop/src-tauri/entitlements.mac.plist"
macos="$app/Contents/MacOS"
[[ -x "$macos/qrtz-app" ]] || { echo "$macos/qrtz-app missing: is $app a qrtz.app built from main?" >&2; exit 1; }

# name identifier entitlements(app | none | repo path). Must match confinement-map.json bundledCode (checked by
# crates/qrtz-lifecycle/tests/offline_graph.rs).
NESTED="
qrtz-helper qrtz-helper app
qrtz ai.qrtz.cli none
qrtz-snap ai.qrtz.snap native/qrtz-helper/entitlements/ai.qrtz.snap.entitlements
"

ent_file() { case "$1" in app) echo "$app_ent" ;; none) echo "" ;; *) echo "$repo/$1" ;; esac; }
keys() { grep -o '<key>[^<]*</key>' | sed 's/<[^>]*>//g' | sort; }

echo "== sign nested code"
while read -r name id ent; do
  [[ -n "$name" ]] || continue
  bin="$macos/$name"
  [[ -f "$bin" ]] || { echo "$bin missing (externalBin in tauri.macos.conf.json?)" >&2; exit 1; }
  if grep -q "qrtz: dev stub" "$bin"; then echo "$bin is a dev stub" >&2; exit 1; fi
  f="$(ent_file "$ent")"
  args=(--force --options runtime --timestamp --identifier "$id" --sign "$identity")
  [[ -n "$f" ]] && args+=(--entitlements "$f")
  codesign "${args[@]}" "$bin"
  echo "  $name: $id, entitlements ${ent}"
done <<<"$NESTED"

echo "== reseal qrtz.app"
codesign --force --options runtime --timestamp --entitlements "$app_ent" --sign "$identity" "$app"

echo "== check"
codesign --verify --strict --deep --verbose=2 "$app"
while read -r name id ent; do
  [[ -n "$name" ]] || continue
  bin="$macos/$name"
  dr="identifier \"$id\" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate leaf[subject.OU] = \"$team\""
  codesign --verify --strict -R="$dr" "$bin"
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
done <<<"$NESTED"

[[ "$notarize" == 1 ]] || { echo "signed; not notarized (--no-notarize)"; exit 0; }

echo "== notarize and staple the app"
# notarize.sh fails unless Apple answers Accepted: `notarytool submit --wait` can exit 0 on a rejected upload.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
ditto -c -k --keepParent --sequesterRsrc "$app" "$work/qrtz.zip"
bash "$repo/scripts/release/notarize.sh" "$work/qrtz.zip" "$profile"
xcrun stapler staple "$app"
spctl --assess --type execute --verbose=2 "$app"

echo "== the .dmg"
version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")"
dmg="$(dirname "$app")/qrtz_${version}_aarch64.dmg"
rm -f "$dmg"
mkdir "$work/dmg" && ditto "$app" "$work/dmg/qrtz.app" && ln -s /Applications "$work/dmg/Applications"
hdiutil create -volname qrtz -srcfolder "$work/dmg" -ov -format UDZO "$dmg" >/dev/null
codesign --force --timestamp --sign "$identity" "$dmg"
bash "$repo/scripts/release/notarize.sh" "$dmg" "$profile"
xcrun stapler staple "$dmg"
xcrun stapler validate "$app" && xcrun stapler validate "$dmg"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
"$macos/qrtz" --help >/dev/null && echo "  qrtz --help: ok"
echo "done: $app and $dmg"
