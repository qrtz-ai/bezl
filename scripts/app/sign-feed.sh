#!/usr/bin/env bash
# ADR 0181 (P3.7): sign a qrtz.app update feed with the app-update key and wrap it into latest.json.
#
#   scripts/app/sign-feed.sh <feed.json> <latest.json to write>
#
# The signature covers the feed's exact bytes (its expires included); it is checked against the public key the app
# has compiled in before anything is written. Needs TAURI_SIGNING_PRIVATE_KEY (the APP_UPDATE_KEY secret; empty
# password), node_modules/.bin/tauri and minisign. Runs in CI only: the private key never leaves GitHub's secrets.
set -euo pipefail
feed="${1:?usage: sign-feed.sh <feed.json> <latest.json>}"
out="${2:?usage: sign-feed.sh <feed.json> <latest.json>}"
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
[[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]] || { echo "sign-feed: TAURI_SIGNING_PRIVATE_KEY is not set" >&2; exit 1; }
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cp "$feed" "$tmp/feed.json"
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" \
  "$repo/node_modules/.bin/tauri" signer sign "$tmp/feed.json" >/dev/null
[[ -s "$tmp/feed.json.sig" ]] || { echo "sign-feed: no signature written" >&2; exit 1; }
grep -v '^#' "$repo/crates/qrtz-app/src/app_update_key.pub" | tr -d '[:space:]' | base64 --decode >"$tmp/k.pub"
base64 --decode <"$tmp/feed.json.sig" >"$tmp/k.sig"
minisign -V -p "$tmp/k.pub" -x "$tmp/k.sig" -m "$tmp/feed.json" >/dev/null
node "$repo/scripts/app/feed.mjs" --wrap "$tmp/feed.json" --wrap-sig "$tmp/feed.json.sig" >"$out"
echo "sign-feed: $out signed ($(node -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1])).expires)' "$tmp/feed.json"))"
