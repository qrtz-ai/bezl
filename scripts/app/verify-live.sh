#!/usr/bin/env bash
# ADR 0181 / QRT-297: what the `index` GitHub release serves for qrtz.app is exactly what was signed and uploaded.
#
#   scripts/app/verify-live.sh <dir with the archive, its .sig and latest.json> <notarized dmg>
#
# The versioned files are compared at once; latest.json and qrtz-latest.dmg are retried for up to 5 minutes.
# Layout path v1/app/x is the asset v1.app.x (scripts/release/gh-store.sh). Needs VERSION. QRTZ_APP_HOST overrides the host.
set -euo pipefail
dir="${1:?usage: verify-live.sh <dir> <dmg>}"
dmg="${2:?usage: verify-live.sh <dir> <dmg>}"
v="${VERSION:?VERSION is not set}"
host="${QRTZ_APP_HOST:-https://github.com/qrtz-ai/qrtz/releases/download/index}"
sha() { if command -v shasum >/dev/null; then shasum -a 256 "$1"; else sha256sum "$1"; fi | cut -d' ' -f1; }
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
tgz="qrtz_${v}_aarch64.app.tar.gz"
same() { # <url path> <local file> [tries]
  local tries="${3:-1}" i
  for ((i = 1; i <= tries; i++)); do
    if curl -fsSL --max-time 600 -H 'Cache-Control: no-cache' -o "$tmp/got" "$host/${1//\//.}" &&
      [[ "$(sha "$tmp/got")" == "$(sha "$2")" ]]; then
      echo "verify-live: $1 ok ($(sha "$2"))"
      return 0
    fi
    [[ $i -lt $tries ]] && sleep 15
  done
  echo "verify-live: $host/${1//\//.} isn't the uploaded file" >&2
  return 1
}
same "v1/app/$v/$tgz" "$dir/$tgz"
same "v1/app/$v/$tgz.sig" "$dir/$tgz.sig"
same "v1/app/$v/qrtz_${v}_aarch64.dmg" "$dmg"
same "v1/app/latest.json" "$dir/latest.json" 20
same "v1/app/qrtz-latest.dmg" "$dmg" 20
echo "verify-live: qrtz $v is live"
