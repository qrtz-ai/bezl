#!/usr/bin/env bash
# CI (ADR 0179): fill a fresh publish output dir with what modules.qrtz.ai serves, so `qrtz-publish build` and
# `timestamp` continue from the live index (they read the current timestamp + index from the dir, and `verify` checks
# every blob the index lists). A runner has no ~/qrtz-publish-out of its own; the bucket is the record.
#
#   scripts/ci/hydrate-out.sh <out dir>     needs R2_ACCOUNT_ID, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY, rclone
#
# Only v1/ is copied (the bucket holds nothing else; model files are never in it, ADR 0171). Root 1 must hash to the
# pin in crates/qrtz-app/src/module_hosts.json and match the public copy in crates/qrtz-modules/roots/.
set -euo pipefail
out="${1:?usage: hydrate-out.sh <out dir>}"
repo="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=r2-env.sh
. "$repo/scripts/ci/r2-env.sh"
bucket="${R2_BUCKET:-qrtz-modules}"
sha() { if command -v shasum >/dev/null; then shasum -a 256 "$1"; else sha256sum "$1"; fi | cut -d' ' -f1; }

mkdir -p "$out/v1"
# ADR 0181: v1/app/ is qrtz.app's own update (app-release.yml); never part of a module publish.
rclone copy "qrtzr2:$bucket/v1" "$out/v1" --exclude "app/**" --transfers 8 --checkers 16 --stats-one-line -v 2>&1 | tail -5
[[ -f "$out/v1/root/1.root.json" ]] || { echo "hydrate: the bucket has no v1/root/1.root.json" >&2; exit 1; }
pin="$(grep -o '"root1Sha256": *"[0-9a-f]\{64\}"' "$repo/crates/qrtz-app/src/module_hosts.json" | grep -o '[0-9a-f]\{64\}')"
[[ "$(sha "$out/v1/root/1.root.json")" == "$pin" ]] || { echo "hydrate: the bucket's root 1 isn't the pinned root $pin" >&2; exit 1; }
for f in "$repo"/crates/qrtz-modules/roots/*.root.json; do
  [[ -f "$f" ]] || continue
  b="$(basename "$f")"
  if [[ -f "$out/v1/root/$b" ]] && ! cmp -s "$f" "$out/v1/root/$b"; then
    echo "hydrate: v1/root/$b in the bucket differs from crates/qrtz-modules/roots/$b" >&2
    exit 1
  fi
done
echo "hydrate: $out mirrors $bucket/v1 ($(find "$out/v1" -type f | wc -l | tr -d ' ') objects)"
