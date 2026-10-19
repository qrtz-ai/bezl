#!/usr/bin/env bash
# One-time (and rotation): put the module-release secrets into GitHub (ADR 0179, docs/release.md "Module releases in
# CI"). Run it yourself in Terminal on the release Mac:
#
#   scripts/ci/setup-secrets.sh                 every secret
#   scripts/ci/setup-secrets.sh cert|notary|keys|r2   just one group (e.g. to rotate it)
#   scripts/ci/setup-secrets.sh app-update      qrtz.app's update key (ADR 0181): made here once, never on a server
#   scripts/ci/setup-secrets.sh --list          what it sets, from where (sets nothing)
#
# Every value goes from its source straight into `gh secret set` on stdin: from the keychain (`security`), from a
# file, or from a hidden prompt. Nothing is printed, put on a command line, or left on disk (the one temp dir is 0700
# and deleted on exit). It never touches the offline root keys (~/qrtz-root-a, the keychain item qrtz-root-b, any
# root-*.key) and refuses to upload one.
#
# Secrets (repo qrtz-ai/qrtz, override with QRTZ_REPO):
#   MACOS_CERT_P12, MACOS_CERT_PASSWORD   only your "Developer ID Application: … (MR26CCZ496)" identity, as a .p12
#                                         with a fresh random password (security export asks to allow it)
#   AC_APPLE_ID, AC_APP_PASSWORD, AC_TEAM_ID   notarization with your Apple ID + an app-specific password (default), or
#   AC_API_KEY, AC_KEY_ID, AC_ISSUER_ID        an App Store Connect API key (.p8)
#   QRTZ_INDEX_KEY, QRTZ_TIMESTAMP_KEY    the ONLINE signing keys publish.sh and timestamp-job.sh use:
#                                         ~/.qrtz-keys/index.key and ~/.qrtz-keys/timestamp.key
#   R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY, R2_ACCOUNT_ID   the keychain item qrtz-r2 (account = access key id)
#   APP_UPDATE_KEY                        `app-update` (not part of `all`): ~/.qrtz-keys/app-update.key, made on this
#                                         Mac by `tauri signer generate` (no password; 0600, like index.key) the first
#                                         time; its public key goes into crates/qrtz-app/src/app_update_key.pub (commit
#                                         it). An existing key is never replaced: it is only uploaded again.
set -euo pipefail
repo="${QRTZ_REPO:-qrtz-ai/qrtz}"
team="MR26CCZ496"
keys="${QRTZ_KEYS:-$HOME/.qrtz-keys}"
r2_account_default="177665085902fbdae8b82993bb7a0b91"
ossl=/usr/bin/openssl # LibreSSL: its .p12 imports into any macOS keychain
die() { echo "setup-secrets: $*" >&2; exit 1; }
say() { echo "setup-secrets: $*"; }

if [[ "${1:-}" == --list ]]; then
  sed -n '/^# Secrets/,/only uploaded again/p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
fi
[[ "$(uname -s)" == Darwin ]] || die "run this on your Mac (it reads the login keychain)"
command -v gh >/dev/null || die "gh not found (brew install gh)"
gh auth status >/dev/null 2>&1 || die "gh isn't signed in: run \`gh auth login\` first"
gh repo view "$repo" --json name >/dev/null 2>&1 || die "gh can't see $repo"

tmp="$(mktemp -d)"
chmod 700 "$tmp"
trap 'rm -rf "$tmp"' EXIT
umask 077

put() { # <name>: the value on stdin, trailing newline dropped
  local v
  v="$(cat)"
  [[ -n "$v" ]] || die "$1: empty, not set"
  printf '%s' "$v" | gh secret set "$1" --repo "$repo" >/dev/null
  say "set $1"
}

# A root key is never a CI secret (ADR 0153: roots stay offline; ADR 0179).
not_root() { # <path>
  local p="$1" real
  real="$(cd "$(dirname "$p")" && pwd -P)/$(basename "$p")"
  [[ -L "$p" ]] && real="$(readlink "$p")"
  case "$real" in
    *qrtz-root* | */root-*.key | root-*.key) die "$p is (or points at) an offline root key: refused" ;;
  esac
}

do_cert() {
  say "Developer ID: exporting only \"Developer ID Application: … ($team)\" from your login keychain."
  say "  macOS asks you to allow the export (your login password); answer that dialog, not here."
  local ids hash
  ids="$(security find-identity -v -p codesigning)"
  hash="$(sed -n "s/^ *[0-9]*) \([0-9A-F]\{40\}\) \"Developer ID Application: .* ($team)\"$/\1/p" <<<"$ids" | head -1)"
  [[ -n "$hash" ]] || die "no valid \"Developer ID Application: … ($team)\" identity in your keychains"
  local allpw newpw
  allpw="$("$ossl" rand -hex 24)"
  newpw="$("$ossl" rand -hex 24)"
  # security export takes every identity of the keychain; only the matching one is repacked below.
  security export -k "$HOME/Library/Keychains/login.keychain-db" -t identities -f pkcs12 -P "$allpw" -o "$tmp/all.p12" >/dev/null
  PW="$allpw" "$ossl" pkcs12 -in "$tmp/all.p12" -passin env:PW -nodes -out "$tmp/all.pem" 2>/dev/null ||
    die "couldn't read the exported identities"
  rm -f "$tmp/all.p12"
  awk -v d="$tmp" '
    /-----BEGIN CERTIFICATE-----/ { c++; f = d "/cert" c ".pem" }
    /-----BEGIN (RSA |EC |ENCRYPTED )?PRIVATE KEY-----/ { k++; f = d "/key" k ".pem" }
    f { print > f }
    /-----END/ { close(f); f = "" }' "$tmp/all.pem"
  rm -f "$tmp/all.pem"
  local cert="" key="" c k pub
  for c in "$tmp"/cert*.pem; do
    [[ -f "$c" ]] || continue
    if [[ "$("$ossl" x509 -in "$c" -noout -fingerprint -sha1 | sed 's/.*=//; s/://g')" == "$hash" ]]; then
      cert="$c"
      break
    fi
  done
  [[ -n "$cert" ]] || die "the export has no certificate $hash"
  pub="$("$ossl" x509 -in "$cert" -noout -pubkey)"
  for k in "$tmp"/key*.pem; do
    [[ -f "$k" ]] || continue
    if [[ "$("$ossl" pkey -in "$k" -pubout 2>/dev/null)" == "$pub" ]]; then
      key="$k"
      break
    fi
  done
  [[ -n "$key" ]] || die "the export has no private key for certificate $hash"
  PW="$newpw" "$ossl" pkcs12 -export -in "$cert" -inkey "$key" -name "Developer ID Application ($team)" \
    -passout env:PW -out "$tmp/one.p12"
  rm -f "$tmp"/cert*.pem "$tmp"/key*.pem
  base64 -i "$tmp/one.p12" | tr -d '\n' | put MACOS_CERT_P12
  printf '%s' "$newpw" | put MACOS_CERT_PASSWORD
  rm -f "$tmp/one.p12"
}

do_notary() {
  say "Notarization: your Mac's qrtz-notary profile can't be copied out of the keychain, so enter its credentials."
  local how
  read -r -p "  1) Apple ID + app-specific password (what qrtz-notary most likely uses)  2) App Store Connect API key [1]: " how
  case "${how:-1}" in
    1)
      local email
      read -r -p "  Apple ID email: " email
      [[ "$email" == *@* ]] || die "not an email address"
      say "  The app-specific password (xxxx-xxxx-xxxx-xxxx). Not saved? Make a new one at https://account.apple.com"
      say "  → Sign-In and Security → App-Specific Passwords (it doesn't break qrtz-notary unless you revoke the old one)."
      local pw
      read -r -s -p "  app-specific password (hidden): " pw
      echo
      [[ -n "$pw" ]] || die "empty password"
      printf '%s' "$email" | put AC_APPLE_ID
      printf '%s' "$pw" | put AC_APP_PASSWORD
      pw=""
      printf '%s' "$team" | put AC_TEAM_ID
      ;;
    2)
      say "  Make the key at https://appstoreconnect.apple.com/access/integrations/api (Users and Access → Integrations"
      say "  → App Store Connect API → Team Keys → +, access Developer); download AuthKey_<id>.p8 (once only)."
      local p8 kid iss
      read -r -p "  path to the .p8: " p8
      p8="${p8/#\~/$HOME}"
      [[ -f "$p8" ]] || die "$p8: no such file"
      grep -q 'BEGIN PRIVATE KEY' "$p8" || die "$p8: not a .p8 key"
      not_root "$p8"
      read -r -p "  Key ID (10 characters): " kid
      read -r -p "  Issuer ID (a UUID, top of the Team Keys page): " iss
      [[ "$kid" =~ ^[A-Z0-9]{10}$ ]] || die "Key ID: 10 letters or digits"
      [[ "$iss" =~ ^[0-9a-fA-F-]{36}$ ]] || die "Issuer ID: a UUID"
      put AC_API_KEY <"$p8"
      printf '%s' "$kid" | put AC_KEY_ID
      printf '%s' "$iss" | put AC_ISSUER_ID
      ;;
    *) die "1 or 2" ;;
  esac
}

do_keys() {
  # publish.sh: "$keys/index.key" (the index) and "$keys/timestamp.key" (the timestamp); timestamp-job.sh only the
  # latter. Both are hex Ed25519 seeds, 0600. Root keys are never read here.
  case "$keys" in *qrtz-root*) die "QRTZ_KEYS=$keys is a root key dir: refused" ;; esac
  local role f
  for role in index timestamp; do
    f="$keys/$role.key"
    [[ -f "$f" ]] || die "$f: not found (the online $role key)"
    not_root "$f"
    grep -Eq '^[0-9a-f]{64}[[:space:]]*$' "$f" || die "$f: not a 64-hex Ed25519 seed"
    tr -d '[:space:]' <"$f" | put "QRTZ_$(tr a-z A-Z <<<"$role")_KEY"
  done
}

do_r2() {
  local id
  id="$(security find-generic-password -s qrtz-r2 2>/dev/null | sed -n 's/^ *"acct"<blob>="\(.*\)"$/\1/p')"
  [[ -n "$id" ]] || die "no keychain item qrtz-r2 (security add-generic-password -s qrtz-r2 -a <access key id> -w)"
  printf '%s' "$id" | put R2_ACCESS_KEY_ID
  security find-generic-password -s qrtz-r2 -w | tr -d '\n' | put R2_SECRET_ACCESS_KEY
  printf '%s' "${R2_ACCOUNT_ID:-$r2_account_default}" | put R2_ACCOUNT_ID
}

do_app_update() {
  # ADR 0181: the key that signs qrtz.app's update archives. Separate from the module root, index and timestamp keys;
  # its public half is compiled into the app. The private key is made here and lives only in this file and in the
  # APP_UPDATE_KEY secret: never printed, never on a command line, never on a server.
  local root key pub out
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  key="$keys/app-update.key"
  pub="$root/crates/qrtz-app/src/app_update_key.pub"
  case "$keys" in *qrtz-root*) die "QRTZ_KEYS=$keys is a root key dir: refused" ;; esac
  mkdir -p "$keys"
  chmod 700 "$keys"
  if [[ -e "$key" ]]; then
    not_root "$key"
    [[ -f "$key.pub" ]] || die "$key exists but $key.pub doesn't: not replacing it (see ADR 0181, rotation)"
    say "$key exists: uploading it again (never replaced here)"
  else
    local tauri="$root/node_modules/.bin/tauri"
    [[ -x "$tauri" ]] || die "no tauri CLI at $tauri (run \`bun install\` in $root first)"
    # --ci -p "": no password (a 0600 file, like index.key). Its output (which names the key) goes nowhere.
    "$tauri" signer generate --ci -p "" -w "$key" >/dev/null 2>&1 || die "tauri signer generate failed"
    [[ -f "$key" && -f "$key.pub" ]] || die "tauri signer generate wrote no key"
  fi
  chmod 600 "$key" "$key.pub"
  tr -d '[:space:]' <"$key" | put APP_UPDATE_KEY
  out="$(tr -d '[:space:]' <"$key.pub")"
  [[ "$out" =~ ^[A-Za-z0-9+/]+=*$ ]] || die "$key.pub isn't a base64 public key"
  {
    echo "# ADR 0181: qrtz.app's update public key (minisign, base64 of the .pub text), compiled into the app."
    echo "# Made by scripts/ci/setup-secrets.sh app-update; the private key is ~/.qrtz-keys/app-update.key and the"
    echo "# APP_UPDATE_KEY secret only. Rotation: ADR 0181."
    echo "$out"
  } >"$pub"
  chmod 644 "$pub"
  say "the app-update public key (also written to crates/qrtz-app/src/app_update_key.pub; commit and push it):"
  echo "$out"
}

case "${1:-all}" in
  all) do_cert; do_notary; do_keys; do_r2 ;;
  cert) do_cert ;;
  notary) do_notary ;;
  keys) do_keys ;;
  r2) do_r2 ;;
  app-update) do_app_update ;;
  *) die "usage: setup-secrets.sh [all|cert|notary|keys|r2|app-update|--list]" ;;
esac
say "done. In $repo: $(gh secret list --repo "$repo" | cut -f1 | tr '\n' ' ')"
say "Run a release: gh workflow run modules-release.yml --repo $repo   (the timestamp: modules-timestamp.yml;"
say "  qrtz.app itself: gh workflow run app-release.yml --repo $repo)"
