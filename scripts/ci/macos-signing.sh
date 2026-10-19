#!/usr/bin/env bash
# CI (ADR 0179): the signing and notarization credentials of a module release, on a GitHub-hosted macOS runner, in a
# temporary keychain that `cleanup` deletes (an always() step). Nothing is printed: secrets come from the
# environment (GitHub masks them), derived values are masked with ::add-mask::, and every file holding one is 0600
# under $RUNNER_TEMP and deleted.
#
#   scripts/ci/macos-signing.sh setup     import the Developer ID identity, store the notary profile, write the keys;
#                                         appends QRTZ_SIGN_ID, QRTZ_NOTARY_PROFILE, QRTZ_NOTARY_KEYCHAIN, QRTZ_KEYS
#                                         to $GITHUB_ENV
#   scripts/ci/macos-signing.sh setup-identity   the same without the index/timestamp keys (app-release, ADR 0181:
#                                         qrtz.app's release needs only the identity and the notary profile)
#   scripts/ci/macos-signing.sh keys      only write the signing keys (QRTZ_KEYS)
#   scripts/ci/macos-signing.sh cleanup   delete the keychain and every secret file (safe to run twice)
#
# Env (GitHub secrets):
#   MACOS_CERT_P12 (base64 .p12 holding only "Developer ID Application: … (MR26CCZ496)"), MACOS_CERT_PASSWORD
#   notarization, one of:  AC_APPLE_ID + AC_APP_PASSWORD (+ AC_TEAM_ID, default MR26CCZ496)   [the default]
#                          AC_API_KEY (.p8 contents) + AC_KEY_ID + AC_ISSUER_ID                [App Store Connect API]
#   QRTZ_INDEX_KEY, QRTZ_TIMESTAMP_KEY (hex Ed25519 seeds; the online keys, never a root key). With `keys`, either
#   may be absent (the timestamp job gets only the timestamp key).
set -euo pipefail
die() { echo "macos-signing: $*" >&2; exit 1; }
tmp="${RUNNER_TEMP:?RUNNER_TEMP is not set (run this in GitHub Actions)}"
kc="$tmp/qrtz-signing.keychain-db"
keys="$tmp/qrtz-keys"
team="${AC_TEAM_ID:-MR26CCZ496}"
genv() { [[ -n "${GITHUB_ENV:-}" ]] && echo "$1=$2" >>"$GITHUB_ENV"; return 0; }

write_keys() {
  umask 077
  rm -rf "$keys"
  mkdir -m 700 "$keys"
  local n=0
  if [[ -n "${QRTZ_INDEX_KEY:-}" ]]; then
    printf '%s\n' "$QRTZ_INDEX_KEY" | tr -d '[:space:]' >"$keys/index.key"
    n=$((n + 1))
  fi
  if [[ -n "${QRTZ_TIMESTAMP_KEY:-}" ]]; then
    printf '%s\n' "$QRTZ_TIMESTAMP_KEY" | tr -d '[:space:]' >"$keys/timestamp.key"
    n=$((n + 1))
  fi
  for f in "$keys"/*.key; do
    [[ -e "$f" ]] || continue
    chmod 600 "$f"
    grep -Eq '^[0-9a-f]{64}$' "$f" || die "$(basename "$f"): not a 64-hex Ed25519 seed (check the secret)"
  done
  [[ "$n" -gt 0 ]] || die "neither QRTZ_INDEX_KEY nor QRTZ_TIMESTAMP_KEY is set"
  genv QRTZ_KEYS "$keys"
  echo "macos-signing: $n signing key(s) written to a 0600 temp dir"
}

case "${1:-}" in
  setup | setup-identity)
    [[ "$(uname -s)" == Darwin ]] || die "setup needs macOS"
    for v in MACOS_CERT_P12 MACOS_CERT_PASSWORD; do [[ -n "${!v:-}" ]] || die "$v is not set"; done
    umask 077
    kcpw="$(openssl rand -hex 24)"
    echo "::add-mask::$kcpw"
    security delete-keychain "$kc" 2>/dev/null || true
    security create-keychain -p "$kcpw" "$kc"
    security set-keychain-settings -lut 21600 "$kc"
    security unlock-keychain -p "$kcpw" "$kc"
    p12="$tmp/qrtz-cert.p12"
    printf '%s' "$MACOS_CERT_P12" | base64 --decode >"$p12"
    chmod 600 "$p12"
    security import "$p12" -k "$kc" -f pkcs12 -P "$MACOS_CERT_PASSWORD" -T /usr/bin/codesign -T /usr/bin/security >/dev/null
    rm -f "$p12"
    # The Developer ID intermediate (public), in case the image lacks it.
    if curl -fsS --max-time 30 -o "$tmp/DeveloperIDG2CA.cer" https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer; then
      security import "$tmp/DeveloperIDG2CA.cer" -k "$kc" >/dev/null 2>&1 || true
      rm -f "$tmp/DeveloperIDG2CA.cer"
    fi
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$kcpw" "$kc" >/dev/null
    # The temp keychain first in the search list (codesign finds the identity there), the login keychain kept.
    existing="$(security list-keychains -d user | tr -d '"' | xargs)"
    # shellcheck disable=SC2086
    security list-keychains -d user -s "$kc" $existing
    id="$(security find-identity -v -p codesigning "$kc" | sed -n "s/.*\"\(Developer ID Application: .* ($team)\)\".*/\1/p" | head -1)"
    [[ -n "$id" ]] || die "MACOS_CERT_P12 has no valid \"Developer ID Application: … ($team)\" identity"
    genv QRTZ_SIGN_ID "$id"
    echo "macos-signing: imported $id"

    # notarytool: a profile in the temp keychain (publish.sh and qrtz-publish use --keychain-profile, plus
    # --keychain from QRTZ_NOTARY_KEYCHAIN). store-credentials validates it against Apple first.
    profile=qrtz-notary
    if [[ -n "${AC_APPLE_ID:-}" ]]; then
      [[ -n "${AC_APP_PASSWORD:-}" ]] || die "AC_APPLE_ID is set but AC_APP_PASSWORD isn't"
      xcrun notarytool store-credentials "$profile" --apple-id "$AC_APPLE_ID" --team-id "$team" \
        --password "$AC_APP_PASSWORD" --keychain "$kc" >/dev/null
      how="Apple ID + app-specific password"
    elif [[ -n "${AC_API_KEY:-}" ]]; then
      for v in AC_KEY_ID AC_ISSUER_ID; do [[ -n "${!v:-}" ]] || die "AC_API_KEY is set but $v isn't"; done
      p8="$tmp/qrtz-AuthKey.p8"
      printf '%s\n' "$AC_API_KEY" >"$p8"
      chmod 600 "$p8"
      xcrun notarytool store-credentials "$profile" --key "$p8" --key-id "$AC_KEY_ID" --issuer "$AC_ISSUER_ID" \
        --keychain "$kc" >/dev/null
      rm -f "$p8"
      how="App Store Connect API key"
    else
      die "no notarization credentials: set AC_APPLE_ID + AC_APP_PASSWORD (or AC_API_KEY + AC_KEY_ID + AC_ISSUER_ID)"
    fi
    genv QRTZ_NOTARY_PROFILE "$profile"
    genv QRTZ_NOTARY_KEYCHAIN "$kc"
    echo "macos-signing: notary profile $profile stored in the temp keychain ($how)"
    [[ "$1" == setup ]] && write_keys
    ;;
  keys) write_keys ;;
  cleanup)
    if [[ "$(uname -s)" == Darwin ]]; then
      security delete-keychain "$kc" 2>/dev/null || true
    fi
    rm -rf "$keys" "$tmp/qrtz-cert.p12" "$tmp/qrtz-AuthKey.p8"
    echo "macos-signing: cleaned up"
    ;;
  *) sed -n '2,20p' "$0"; exit 2 ;;
esac
