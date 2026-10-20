#!/usr/bin/env bash
# CI (ADR 0179, QRT-297): after an upload, check that the `index` GitHub release serves exactly what was signed. Every object of the
# output dir (qrtz-publish upload-list: root, index, blobs, timestamp pair) is fetched from the public host into a
# fresh dir and compared by sha256 with the local copy; then `qrtz-publish verify` checks the fresh dir as a new
# client would (pinned root, root chain, fresh timestamp, index signature, every blob's size and hash).
#
#   scripts/ci/verify-live.sh <out dir> [--timestamp-only]    env: QRTZ_PUBLISH_BIN (built qrtz-publish),
#                                                             QRTZ_PRIMARY_URL (the index release)
# Layout path v1/a/b is the asset v1.a.b. The timestamp pair is retried for up to ~6 minutes.
set -euo pipefail
out="${1:?usage: verify-live.sh <out dir> [--timestamp-only]}"
mode="${2:-}"
repo="$(cd "$(dirname "$0")/../.." && pwd)"
bin="${QRTZ_PUBLISH_BIN:-${CARGO_TARGET_DIR:-$repo/target}/release/qrtz-publish}"
host="${QRTZ_PRIMARY_URL:-https://github.com/qrtz-ai/qrtz/releases/download/index}"
sha() { if command -v shasum >/dev/null; then shasum -a 256 "$1"; else sha256sum "$1"; fi | cut -d' ' -f1; }
fresh="$(mktemp -d)"
trap 'rm -rf "$fresh"' EXIT

fetch() { # <key> <dest>: the public host's copy (the flat asset, following GitHub's redirect)
  curl -fsSL --retry 3 --max-time 120 -H 'Cache-Control: no-cache' -o "$2" "$host/${1//\//.}"
}

n=0
bad=0
while IFS=$'\t' read -r key _size _cc; do
  mkdir -p "$(dirname "$fresh/$key")"
  want="$(sha "$out/$key")"
  tries=1
  [[ "$key" == v1/timestamp.json* ]] && tries=24
  ok=0
  for ((i = 1; i <= tries; i++)); do
    if fetch "$key" "$fresh/$key" && [[ "$(sha "$fresh/$key")" == "$want" ]]; then
      ok=1
      break
    fi
    ((i < tries)) && sleep 15
  done
  if [[ "$ok" == 1 ]]; then
    n=$((n + 1))
  else
    echo "verify-live: $host/$key doesn't serve the signed bytes (want sha256 $want)" >&2
    bad=$((bad + 1))
  fi
done < <("$bin" upload-list --out "$out")
[[ "$bad" == 0 ]] || { echo "verify-live: $bad object(s) wrong or missing on $host" >&2; exit 1; }

pin="$(grep -o '"root1Sha256": *"[0-9a-f]\{64\}"' "$repo/crates/qrtz-app/src/module_hosts.json" | grep -o '[0-9a-f]\{64\}')"
"$bin" verify --out "$fresh" --root-sha256 "$pin"
ix="$(grep -o '"sha256": *"[0-9a-f]\{64\}"' "$fresh/v1/timestamp.json" | grep -o '[0-9a-f]\{64\}' | head -1)"
echo "verify-live: $host serves all $n signed objects; live index $ix verifies against root 1 ($pin)${mode:+ ($mode)}"
