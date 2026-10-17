#!/usr/bin/env bash
# Signed + notarized macOS release build of qrtz. Run on a Mac with Xcode, Node 22 and a
# "Developer ID Application" certificate in the login keychain. See docs/release.md.
#
# Required:
#   QRTZ_MAC_SIGN_IDENTITY   e.g. "Developer ID Application: Jared Grimes (TEAMID1234)"
# Notarization, one of:
#   QRTZ_NOTARY_PROFILE      a `xcrun notarytool store-credentials` keychain profile (preferred)
#   APPLE_ID + APPLE_APP_SPECIFIC_PASSWORD + APPLE_TEAM_ID
# Optional:
#   QRTZ_ARCH                arm64 (default) | x64 | both   (x64: no on-device embeddings, FTS only)
#   QRTZ_SKIP_HELPER=1       reuse an existing native/qrtz-helper build
set -euo pipefail
cd "$(dirname "$0")/.."

die() { echo "release-mac: $*" >&2; exit 1; }
[[ "$(uname)" == "Darwin" ]] || die "must run on macOS"
[[ -n "${QRTZ_MAC_SIGN_IDENTITY:-}" ]] || die "set QRTZ_MAC_SIGN_IDENTITY (security find-identity -v -p codesigning)"
security find-identity -v -p codesigning | grep -qF "$QRTZ_MAC_SIGN_IDENTITY" \
  || die "identity not found in keychain: $QRTZ_MAC_SIGN_IDENTITY"
if [[ -z "${QRTZ_NOTARY_PROFILE:-}" ]]; then
  [[ -n "${APPLE_ID:-}" && -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]] \
    || die "set QRTZ_NOTARY_PROFILE, or APPLE_ID + APPLE_APP_SPECIFIC_PASSWORD + APPLE_TEAM_ID"
fi
node -e 'process.exit(+process.versions.node.split(".")[0] >= 22 ? 0 : 1)' || die "Node 22+ required"

if [[ -n "${QRTZ_NOTARY_PROFILE:-}" ]]; then
  notary=(--keychain-profile "$QRTZ_NOTARY_PROFILE")
else
  notary=(--apple-id "$APPLE_ID" --password "$APPLE_APP_SPECIFIC_PASSWORD" --team-id "$APPLE_TEAM_ID")
fi

case "${QRTZ_ARCH:-arm64}" in
  arm64) archs=(arm64) ;;
  x64) archs=(x64) ;;
  both) archs=(arm64 x64) ;;
  *) die "QRTZ_ARCH must be arm64, x64 or both" ;;
esac

echo "==> checks"
npm ci
npm run check

if [[ "${QRTZ_SKIP_HELPER:-}" != "1" ]]; then
  echo "==> Swift helper (release)"
  npm run build:helper
  npm run test:helper
fi

for arch in "${archs[@]}"; do
  [[ "$arch" == x64 ]] && echo "note: onnxruntime-node ships no darwin/x64 binary; x64 build uses FTS-only chat retrieval"
  echo "==> make (signed + notarized) darwin/$arch"
  npm run make -w @qrtz/desktop -- --platform=darwin --arch="$arch"

  app="apps/desktop/out/qrtz-darwin-$arch/qrtz.app"
  echo "==> verify $app"
  codesign --verify --deep --strict --verbose=2 "$app"
  codesign -d --entitlements - "$app" | grep -q audio-input || die "entitlements missing"
  codesign -d --verbose=2 "$app" 2>&1 | grep -q "flags=.*runtime" || die "hardened runtime missing"
  helper="$app/Contents/Resources/qrtz-helper"
  [[ -f "$helper" ]] && codesign --verify --strict --verbose=2 "$helper"
  xcrun stapler validate "$app"
  spctl --assess --type execute --verbose=2 "$app"
  for dmg in apps/desktop/out/make/*"$arch"*.dmg; do
    [[ -f "$dmg" ]] || continue
    xcrun notarytool submit "$dmg" "${notary[@]}" --wait
    xcrun stapler staple "$dmg"
  done
done

echo "==> artifacts"
find apps/desktop/out/make -type f \( -name '*.dmg' -o -name '*.zip' \) -print
echo "Done. Publish the zip + dmg as a GitHub release (see docs/release.md, auto-update)."
