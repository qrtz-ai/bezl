#!/usr/bin/env bash
# Publish the package index to modules.qrtz.ai (Cloudflare R2) and its B2 mirror (ADR 0153, ADR 0157,
# docs/modules/production-setup.md). Runs on the release Mac, which holds ~/.qrtz-keys (index + timestamp keys).
#
#   scripts/modules/publish.sh                 build, verify, upload (R2 then B2), check the live hosts
#   scripts/modules/publish.sh --no-upload     build and verify only (safe to run any time)
#   scripts/modules/publish.sh --timestamp     re-sign the timestamp and upload only it (at least weekly)
#
# What gets published: the pinned model packages found under $QRTZ_MODELS_DIR (the app's own downloaded copies, each
# file checked against its pin), plus any built extension/service package dirs in $QRTZ_PUBLISH_PACKAGES. Published
# objects are immutable: rclone --immutable refuses to change one, nothing is ever deleted, and the timestamp goes
# last so a client never sees an index whose blobs aren't up yet.
#
# Environment:
#   R2_ACCOUNT_ID, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY   the R2 API token (Object Read & Write, bucket only)
#   B2_KEY_ID, B2_APPLICATION_KEY                            the B2 application key (bucket only)
#   QRTZ_TEAM_ID        the Developer ID team (10 characters; the publisher team of every package, models too;
#                       default MR26CCZ496)
#   optional: R2_BUCKET (qrtz-modules), B2_BUCKET (qrtz-modules-mirror), QRTZ_KEYS (~/.qrtz-keys),
#   QRTZ_PUBLISH_OUT (~/qrtz-publish-out), QRTZ_MODELS_DIR (~/Library/Application Support/ai.qrtz.desktop/models),
#   QRTZ_PUBLISH_MODELS (space-separated package names; default: every pinned model whose dir exists),
#   QRTZ_PUBLISH_PACKAGES (space-separated built package dirs, each with its qrtz.json; default none),
#   QRTZ_ROOT1_SHA256 (default: the pin in crates/qrtz-app/src/module_hosts.json, else $QRTZ_PUBLISH_OUT/ROOT1_SHA256),
#   QRTZ_PUBLISH_BIN (a built qrtz-publish; default: cargo build --release -p qrtz-publish),
#   QRTZ_PRIMARY_URL (https://modules.qrtz.ai), QRTZ_MIRROR_URL (from module_hosts.json) for the live check.
# Secrets are read from the environment only (e.g. `export R2_SECRET_ACCESS_KEY=$(security find-generic-password
# -s qrtz-r2 -w)`); nothing here writes them to disk or prints them. rclone gets them as RCLONE_CONFIG_* variables,
# so there is no rclone.conf with keys in it.
set -euo pipefail

die() {
  echo "publish: $*" >&2
  exit 1
}
mode=all
case "${1:-}" in
  "") ;;
  --no-upload) mode=build ;;
  --timestamp) mode=timestamp ;;
  -h | --help)
    sed -n '2,32p' "$0"
    exit 0
    ;;
  *) die "unknown option $1 (--no-upload, --timestamp)" ;;
esac

repo="$(cd "$(dirname "$0")/../.." && pwd)"
keys="${QRTZ_KEYS:-$HOME/.qrtz-keys}"
out="${QRTZ_PUBLISH_OUT:-$HOME/qrtz-publish-out}"
models_dir="${QRTZ_MODELS_DIR:-$HOME/Library/Application Support/ai.qrtz.desktop/models}"
hosts_json="$repo/crates/qrtz-app/src/module_hosts.json"
r2_bucket="${R2_BUCKET:-qrtz-modules}"
b2_bucket="${B2_BUCKET:-qrtz-modules-mirror}"

[[ -f "$keys/timestamp.key" ]] || die "$keys: no timestamp key (run scripts/modules/release-keys.sh first)"
[[ "$mode" == timestamp || -f "$keys/index.key" ]] || die "$keys: no index key (run scripts/modules/release-keys.sh first)"
[[ -f "$out/v1/root/1.root.json" ]] || die "$out: no root 1 (run scripts/modules/release-keys.sh first)"

pin="${QRTZ_ROOT1_SHA256:-}"
if [[ -z "$pin" ]]; then
  pin="$(grep -o '"root1Sha256": *"[0-9a-f]\{64\}"' "$hosts_json" | grep -o '[0-9a-f]\{64\}' || true)"
fi
if [[ -z "$pin" && -f "$out/ROOT1_SHA256" ]]; then
  pin="$(tr -d '[:space:]' <"$out/ROOT1_SHA256")"
  echo "publish: note: module_hosts.json has no root1Sha256 yet; verifying against $out/ROOT1_SHA256" >&2
fi
[[ "$pin" =~ ^[0-9a-f]{64}$ ]] || die "no root1Sha256 (module_hosts.json, QRTZ_ROOT1_SHA256 or $out/ROOT1_SHA256)"
if [[ "$(shasum -a 256 "$out/v1/root/1.root.json" 2>/dev/null || sha256sum "$out/v1/root/1.root.json")" != "$pin"* ]]; then
  die "$out/v1/root/1.root.json doesn't match root1Sha256 $pin"
fi

bin="${QRTZ_PUBLISH_BIN:-}"
if [[ -z "$bin" ]]; then
  (cd "$repo" && cargo build --release -p qrtz-publish)
  bin="${CARGO_TARGET_DIR:-$repo/target}/release/qrtz-publish"
fi
[[ -x "$bin" ]] || die "$bin: not found"

if [[ "$mode" == timestamp ]]; then
  "$bin" timestamp --out "$out" --keys "$keys"
else
  # The models pinned in the app, by package name → the app's dir name for that set (local_models.json and
  # ctrl_laya_model.json; the publisher checks every file against its pin, so a wrong dir fails, it can't mislead).
  # (A function, not an associative array: macOS's /bin/bash is 3.2.)
  model_dir() {
    case "$1" in
      model.parakeet-tdt-v2-en) echo parakeet-tdt-v2 ;;
      model.nemotron) echo nemotron-560ms ;;
      model.nemotron-diarizer) echo nemotron3-diarizer ;;
      model.localvqe) echo localvqe ;;
      model.laya) echo laya ;;
      *) echo "" ;;
    esac
  }
  wanted="${QRTZ_PUBLISH_MODELS:-}"
  if [[ -z "$wanted" ]]; then
    for n in model.parakeet-tdt-v2-en model.nemotron model.nemotron-diarizer model.localvqe model.laya; do
      [[ -d "$models_dir/$(model_dir "$n")" ]] && wanted="$wanted $n"
    done
  fi
  team="${QRTZ_TEAM_ID:-MR26CCZ496}"
  [[ "$team" =~ ^[A-Z0-9]{10}$ && "$team" != 0000000000 ]] || die "QRTZ_TEAM_ID: your 10-character Developer ID team id"

  spec="$(mktemp -d)/publish.json"
  {
    printf '{\n  "teamId": "%s",\n  "pinnedModels": [\n' "$team"
    printf '    "%s",\n    "%s"\n  ],\n' "$repo/crates/qrtz-app/src/local_models.json" "$repo/crates/qrtz-app/src/ctrl_laya_model.json"
    printf '  "models": ['
    sep=""
    for n in $wanted; do
      d="$(model_dir "$n")"
      [[ -n "$d" ]] || die "QRTZ_PUBLISH_MODELS: $n is not a pinned model"
      [[ -d "$models_dir/$d" ]] || die "$models_dir/$d: missing (download $n in the app first)"
      printf '%s\n    { "name": "%s", "dir": "%s" }' "$sep" "$n" "$models_dir/$d"
      sep=","
    done
    printf '\n  ],\n  "packages": ['
    sep=""
    for p in ${QRTZ_PUBLISH_PACKAGES:-}; do
      [[ -f "$p/qrtz.json" ]] || die "$p: not a built package dir (no qrtz.json)"
      printf '%s\n    { "dir": "%s" }' "$sep" "$(cd "$p" && pwd)"
      sep=","
    done
    printf '\n  ]\n}\n'
  } >"$spec"
  echo "publish: spec $spec"
  echo "publish: models:${wanted:- none}; packages: ${QRTZ_PUBLISH_PACKAGES:-none}"
  "$bin" build --out "$out" --spec "$spec" --keys "$keys"
fi
"$bin" verify --out "$out" --root-sha256 "$pin"

[[ "$mode" == build ]] && {
  echo "publish: built and verified in $out (not uploaded)"
  exit 0
}

command -v rclone >/dev/null || die "rclone not found (brew install rclone)"
for v in R2_ACCOUNT_ID R2_ACCESS_KEY_ID R2_SECRET_ACCESS_KEY B2_KEY_ID B2_APPLICATION_KEY; do
  [[ -n "${!v:-}" ]] || die "$v is not set"
done
# rclone remotes from the environment only (no config file with secrets).
export RCLONE_CONFIG_QRTZR2_TYPE=s3 RCLONE_CONFIG_QRTZR2_PROVIDER=Cloudflare
export RCLONE_CONFIG_QRTZR2_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID" RCLONE_CONFIG_QRTZR2_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY"
export RCLONE_CONFIG_QRTZR2_ENDPOINT="https://$R2_ACCOUNT_ID.r2.cloudflarestorage.com"
export RCLONE_CONFIG_QRTZR2_NO_CHECK_BUCKET=true RCLONE_CONFIG_QRTZR2_ACL=private
export RCLONE_CONFIG_QRTZB2_TYPE=b2 RCLONE_CONFIG_QRTZB2_ACCOUNT="$B2_KEY_ID" RCLONE_CONFIG_QRTZB2_KEY="$B2_APPLICATION_KEY"

ts='v1/timestamp.json*'
upload() { # <remote:bucket>
  if [[ "$mode" != timestamp ]]; then
    # Everything but the timestamp: immutable, cached for a year.
    rclone copy "$out" "$1" --immutable --exclude "$ts" --exclude ROOT1_SHA256 \
      --header-upload "Cache-Control: public, max-age=31536000, immutable" --transfers 4 --s3-chunk-size 64M -v
  fi
  # The timestamp last (its .sig first is fine: the client fetches the pair), cached 5 minutes.
  rclone copy "$out" "$1" --include "$ts" --header-upload "Cache-Control: public, max-age=300" --ignore-times -v
}
upload "qrtzr2:$r2_bucket"
upload "qrtzb2:$b2_bucket"

# The live check: what the hosts serve is what was signed.
primary="${QRTZ_PRIMARY_URL:-https://modules.qrtz.ai}"
mirror="${QRTZ_MIRROR_URL:-$(grep -o 'https://[a-z0-9.-]*backblazeb2\.com' "$hosts_json" | head -1)}"
want="$(shasum -a 256 "$out/v1/timestamp.json" 2>/dev/null | cut -d' ' -f1 || sha256sum "$out/v1/timestamp.json" | cut -d' ' -f1)"
for h in "$primary" "$mirror"; do
  got="$(curl -fsS -H 'Cache-Control: no-cache' "$h/v1/timestamp.json?nocache=$RANDOM" | (shasum -a 256 2>/dev/null || sha256sum) | cut -d' ' -f1 || true)"
  if [[ "$got" == "$want" ]]; then
    echo "publish: $h serves the new timestamp"
  else
    echo "publish: WARNING: $h doesn't serve the new timestamp yet (cache or DNS); check $h/v1/timestamp.json in a few minutes" >&2
  fi
done
echo "publish: done"
