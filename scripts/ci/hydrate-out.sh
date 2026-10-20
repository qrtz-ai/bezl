#!/usr/bin/env bash
# CI (ADR 0179, QRT-297): fill a fresh publish output dir with what the package host (the `index` GitHub release)
# serves, so `qrtz-publish build` and `timestamp` continue from the live index (they read the current timestamp +
# index from the dir, and `verify` checks every blob the index lists). A runner has no ~/qrtz-publish-out of its own;
# the release is the record.
#
#   scripts/ci/hydrate-out.sh <out dir>     needs gh (GH_TOKEN); scripts/release/gh-store.sh hydrate
#
# Only module objects are copied (never v1/app/; model files are never on the host, ADR 0171). The public roots in
# crates/qrtz-modules/roots/ fill in any root the release doesn't have yet (the first publish to GitHub). Root 1
# must hash to the pin in crates/qrtz-app/src/module_hosts.json and match the public copy.
set -euo pipefail
out="${1:?usage: hydrate-out.sh <out dir>}"
repo="$(cd "$(dirname "$0")/../.." && pwd)"
sha() { if command -v shasum >/dev/null; then shasum -a 256 "$1"; else sha256sum "$1"; fi | cut -d' ' -f1; }

mkdir -p "$out/v1"
bash "$repo/scripts/release/gh-store.sh" hydrate "$out"
mkdir -p "$out/v1/root"
for f in "$repo"/crates/qrtz-modules/roots/*.root.json "$repo"/crates/qrtz-modules/roots/*.root.json.sig; do
  if [[ -f "$f" && ! -f "$out/v1/root/$(basename "$f")" ]]; then cp "$f" "$out/v1/root/"; fi
done
[[ -f "$out/v1/root/1.root.json" ]] || { echo "hydrate: no v1/root/1.root.json" >&2; exit 1; }
pin="$(grep -o '"root1Sha256": *"[0-9a-f]\{64\}"' "$repo/crates/qrtz-app/src/module_hosts.json" | grep -o '[0-9a-f]\{64\}')"
[[ "$(sha "$out/v1/root/1.root.json")" == "$pin" ]] || { echo "hydrate: the host's root 1 isn't the pinned root $pin" >&2; exit 1; }
for f in "$repo"/crates/qrtz-modules/roots/*.root.json; do
  [[ -f "$f" ]] || continue
  b="$(basename "$f")"
  if [[ -f "$out/v1/root/$b" ]] && ! cmp -s "$f" "$out/v1/root/$b"; then
    echo "hydrate: v1/root/$b on the host differs from crates/qrtz-modules/roots/$b" >&2
    exit 1
  fi
done
echo "hydrate: $out mirrors the index release ($(find "$out/v1" -type f | wc -l | tr -d ' ') objects)"
