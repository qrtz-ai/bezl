#!/usr/bin/env bash
# CI (ADR 0179): decide whether modules-release.yml publishes, before a Mac runner starts. Prints the plan and writes
# `go=0|1` and `force=0|1` to $GITHUB_OUTPUT (or stdout).
#
#   scripts/ci/release-plan.sh [--force]        env: QRTZ_PRIMARY_URL (https://modules.qrtz.ai)
#
# Per module (nano, ctrl, snap), from `version.mjs plan`, its tags and the live index:
#   bump      changed since its <m>-v<x.y.z> tag: version.mjs bumps it                         → go
#   pending   its version has no tag and isn't live (an earlier bump whose publish didn't finish) → go
#   live      its version is live but untagged (published, the tag step didn't run)             → refuse: tag it first
#   released  its version is tagged
# publish.sh --release rebuilds and republishes all three packages, and a rebuilt binary never has the same bytes, so
# a release where some module is unchanged would hit "already published with other content". Until publish.sh only
# publishes the modules it bumped, such a release bumps all three (force=1).
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo"
host="${QRTZ_PRIMARY_URL:-https://modules.qrtz.ai}"
force_in=""
[[ "${1:-}" == --force ]] && force_in=--force
emit() { if [[ -n "${GITHUB_OUTPUT:-}" ]]; then echo "$1" >>"$GITHUB_OUTPUT"; fi; echo "plan: $1"; }

live="$(mktemp)"
trap 'rm -f "$live"' EXIT
ts="$(curl -fsS --max-time 20 "$host/v1/timestamp.json?plan=$RANDOM" 2>/dev/null || true)"
if [[ -n "$ts" ]]; then
  sha="$(grep -o '"sha256": *"[0-9a-f]\{64\}"' <<<"$ts" | grep -o '[0-9a-f]\{64\}' | head -1)"
  curl -fsS --max-time 60 "$host/v1/index/$sha.json" |
    node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{for(const[n,v]of Object.entries(JSON.parse(s).packages))for(const x of Object.keys(v))console.log(n+"@"+x)})' >"$live"
fi
is_live() { grep -qx "$1@$2" "$live"; }

go=0
rebuilt_unchanged=0
bad=""
while IFS=$'\t' read -r m from to; do
  [[ -n "$m" ]] || continue
  if [[ "$to" != - ]]; then
    state="bump $from -> $to"
    go=1
  elif git rev-parse -q --verify "refs/tags/$m-v$from" >/dev/null; then
    state="released ($m-v$from)"
    rebuilt_unchanged=1
  elif is_live "$m" "$from"; then
    state="live but untagged ($m@$from): tag the commit it was built from with $m-v$from first"
    bad="$bad $m"
  else
    state="pending ($from bumped, not published yet)"
    go=1
  fi
  echo "plan: $m: $state"
done < <(node scripts/modules/version.mjs plan $force_in)

if [[ -n "$bad" ]]; then
  echo "::error::$bad: published but untagged; a rebuild would conflict with the live version. Tag it, then rerun."
  emit go=0
  emit force=0
  exit 1
fi
force=0
if [[ "$go" == 1 && "$rebuilt_unchanged" == 1 && -z "$force_in" ]]; then
  echo "::notice::some modules are unchanged; publish.sh --release republishes all three, so every module is bumped"
  force=1
fi
[[ -n "$force_in" ]] && force=1
[[ "$go" == 1 ]] || echo "::notice::no module changed since its release tag; nothing to publish"
emit "go=$go"
emit "force=$force"
