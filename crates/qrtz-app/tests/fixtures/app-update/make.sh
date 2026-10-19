#!/usr/bin/env bash
# Regenerate these TEST ONLY fixtures (README.md) with throwaway keys made here and deleted at the end.
#   TAURI=<path to node_modules/.bin/tauri> crates/qrtz-app/tests/fixtures/app-update/make.sh
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../../../.." && pwd)"
tauri="${TAURI:-$repo/node_modules/.bin/tauri}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cd "$here"
"$tauri" signer generate --ci -p "" -w "$tmp/test.key" >/dev/null 2>&1
"$tauri" signer generate --ci -p "" -w "$tmp/other.key" >/dev/null 2>&1
cp "$tmp/test.key.pub" test.pub
cp "$tmp/other.key.pub" other.pub
sign() { # $1 = key name, $2 = file, $3 = output, [$4 = app version]
  cp "$2" "$tmp/in"
  "$tauri" signer sign -f "$tmp/$1.key" -p "" ${4:+--app-version "$4"} "$tmp/in" >/dev/null 2>&1
  mv "$tmp/in.sig" "$3"
}
sign test artifact.tar.gz good-0.3.0.sig 0.3.0
sign test artifact.tar.gz old-0.2.5.sig 0.2.5
sign other artifact.tar.gz otherkey-0.3.0.sig 0.3.0
fresh=2026-11-15T00:00:00Z
feed() { # $1 = name, $2 = version, $3 = archive sig, [$4 = expires], [$5 = feed key]
  node "$repo/scripts/app/feed.mjs" --version "$2" --archive artifact.tar.gz --sig "$3" \
    --expires "${4:-$fresh}" --notes-file <(echo "faster dictation") >"$tmp/$1.json"
  sign "${5:-test}" "$tmp/$1.json" "$tmp/$1.json.sig"
  node "$repo/scripts/app/feed.mjs" --wrap "$tmp/$1.json" --wrap-sig "$tmp/$1.json.sig" >"feed-$1.json"
}
feed 0.3.0 0.3.0 good-0.3.0.sig
feed 0.3.0-otherkey-archive 0.3.0 otherkey-0.3.0.sig
feed 0.3.0-old-archive-sig 0.3.0 old-0.2.5.sig
feed 0.3.0-expired 0.3.0 good-0.3.0.sig 2026-10-01T00:00:00Z
feed 0.3.0-otherkey-feed 0.3.0 good-0.3.0.sig "$fresh" other
feed 0.2.5 0.2.5 old-0.2.5.sig
feed 0.2.0 0.2.0 good-0.3.0.sig
feed 0.1.9 0.1.9 good-0.3.0.sig
echo "fixtures: regenerated (keys deleted)"
