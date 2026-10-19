#!/usr/bin/env bash
# Publish the package index to modules.qrtz.ai (Cloudflare R2; ADR 0153, ADR 0157, ADR 0167, ADR 0171,
# docs/modules/production-setup.md). Sole package host: no Backblaze B2 mirror. Runs on the release Mac, which
# holds ~/.qrtz-keys (index + timestamp keys).
#
#   scripts/modules/publish.sh                          build, verify, upload (R2 via rclone), check the live host
#   scripts/modules/publish.sh --out-only               build and verify, write the upload list; no upload, no R2
#                                                       token, no rclone (upload the listed small files yourself,
#                                                       e.g. through the Cloudflare connector). --no-upload = same.
#   scripts/modules/publish.sh --timestamp              re-sign the timestamp and upload only it (at least weekly)
#   scripts/modules/publish.sh --timestamp --out-only   re-sign it and list only the timestamp pair
#
# ADR 0171: modules.qrtz.ai hosts module packages, the root, the index and the timestamp only. Model files are never
# uploaded: the signed index lists each one's sha256, size and pinned Hugging Face URL (a commit, never `main`) and
# the app downloads it from Hugging Face and checks it against those. To hash a model set the script uses the app's
# downloaded copy under $QRTZ_MODELS_DIR if there is one, else downloads the set once from Hugging Face (pinned URLs,
# resumable, each file checked against its pin) into $QRTZ_MODEL_CACHE. The model's small qrtz.json and tree are
# published like any package. Built extension/service package dirs come from $QRTZ_PUBLISH_PACKAGES. Published
# objects are immutable: rclone --immutable refuses to change one, nothing is ever deleted, and the timestamp goes
# last so a client never sees an index whose blobs aren't up yet.
#
# The upload list ($QRTZ_PUBLISH_OUT.upload.tsv, beside the output dir so it is never uploaded itself) has one
# `key<TAB>size<TAB>cache-control<TAB>state` line per object in upload order, the timestamp's .sig then the
# timestamp last; state is `new` (not served by $QRTZ_PRIMARY_URL yet) or `live` (already there, immutable: skip).
# Upload each `new` key from $QRTZ_PUBLISH_OUT/<key> to the bucket under the same key with that Cache-Control.
#
# Environment:
#   R2_ACCOUNT_ID, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY   the R2 API token (Object Read & Write, bucket only;
#                       not needed with --out-only)
#   QRTZ_TEAM_ID        the Developer ID team (10 characters; the publisher team of every package, models too;
#                       default MR26CCZ496)
#   optional: R2_BUCKET (qrtz-modules), QRTZ_KEYS (~/.qrtz-keys),
#   QRTZ_PUBLISH_OUT (~/qrtz-publish-out), QRTZ_MODELS_DIR (~/Library/Application Support/ai.qrtz.desktop/models),
#   QRTZ_MODEL_CACHE (~/qrtz-publish-models: where a set missing from QRTZ_MODELS_DIR is downloaded once to hash it),
#   QRTZ_PUBLISH_MODELS (space-separated package names; default: every pinned model whose dir exists; name one to
#                       download and publish it),
#   QRTZ_PUBLISH_PACKAGES (space-separated built package dirs, each with its qrtz.json; default none; built by
#                       scripts/modules/build-packages.sh; each one's code is notarized first, ADR 0170),
#   QRTZ_NOTARY_PROFILE (the notarytool keychain profile, default qrtz-notary),
#   QRTZ_ROOT1_SHA256 (default: the pin in crates/qrtz-app/src/module_hosts.json, else $QRTZ_PUBLISH_OUT/ROOT1_SHA256),
#   QRTZ_PUBLISH_BIN (a built qrtz-publish; default: cargo build --release -p qrtz-publish),
#   QRTZ_PRIMARY_URL (https://modules.qrtz.ai) for the live check.
# Secrets are read from the environment only (e.g. `export R2_SECRET_ACCESS_KEY=$(security find-generic-password
# -s qrtz-r2 -w)`); nothing here writes them to disk or prints them. rclone gets them as RCLONE_CONFIG_* variables,
# so there is no rclone.conf with keys in it.
set -euo pipefail

die() {
  echo "publish: $*" >&2
  exit 1
}
mode=all
out_only=0
for a in "$@"; do
  case "$a" in
    --no-upload | --out-only) out_only=1 ;;
    --timestamp) mode=timestamp ;;
    -h | --help)
      sed -n '2,50p' "$0"
      exit 0
      ;;
    *) die "unknown option $a (--out-only, --no-upload, --timestamp)" ;;
  esac
done

repo="$(cd "$(dirname "$0")/../.." && pwd)"
keys="${QRTZ_KEYS:-$HOME/.qrtz-keys}"
out="${QRTZ_PUBLISH_OUT:-$HOME/qrtz-publish-out}"
models_dir="${QRTZ_MODELS_DIR:-$HOME/Library/Application Support/ai.qrtz.desktop/models}"
hosts_json="$repo/crates/qrtz-app/src/module_hosts.json"
r2_bucket="${R2_BUCKET:-qrtz-modules}"

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
  pins="$repo/crates/qrtz-app/src/local_models.json,$repo/crates/qrtz-app/src/ctrl_laya_model.json"
  cache="${QRTZ_MODEL_CACHE:-$HOME/qrtz-publish-models}"
  # ADR 0171: a set not on this Mac is downloaded once from its pinned Hugging Face URLs, only to hash it (the
  # publisher checks every file against its pin again). Resumable (curl -C -), retried, and a file whose sha256
  # isn't the pin is deleted and fails the run. Prints the dir.
  fetch_model() { # <package> <dir name>
    local n="$1" d="$cache/$2" url path size sha got
    "$bin" model-files --pins "$pins" --package "$n" >"$cache.$n.tsv" || return 1
    while IFS=$'\t' read -r url path size sha; do
      mkdir -p "$(dirname "$d/$path")"
      if [[ -f "$d/$path" && "$(wc -c <"$d/$path" | tr -d ' ')" == "$size" ]]; then
        continue
      fi
      echo "publish: $n: downloading $path ($size bytes) from Hugging Face" >&2
      curl -fL --retry 5 --retry-delay 2 -C - -o "$d/$path.part" "$url" >&2 || return 1
      got="$(shasum -a 256 "$d/$path.part" 2>/dev/null | cut -d' ' -f1 || sha256sum "$d/$path.part" | cut -d' ' -f1)"
      if [[ "$got" != "$sha" ]]; then
        rm -f "$d/$path.part"
        echo "publish: $url: sha256 $got, pinned $sha (refused)" >&2
        return 1
      fi
      mv "$d/$path.part" "$d/$path"
    done <"$cache.$n.tsv"
    rm -f "$cache.$n.tsv"
    echo "$d"
  }
  mkdir -p "$cache"
  wanted="${QRTZ_PUBLISH_MODELS:-}"
  if [[ -z "$wanted" ]]; then
    for n in model.parakeet-tdt-v2-en model.nemotron model.nemotron-diarizer model.localvqe model.laya; do
      [[ -d "$models_dir/$(model_dir "$n")" ]] && wanted="$wanted $n"
    done
  fi
  team="${QRTZ_TEAM_ID:-MR26CCZ496}"
  [[ "$team" =~ ^[A-Z0-9]{10}$ && "$team" != 0000000000 ]] || die "QRTZ_TEAM_ID: your 10-character Developer ID team id"

  # ADR 0170: an extension's or service's code enters the index only notarized. Each package's executable (the
  # extension's entry, or the service bundle's Contents/MacOS executable) goes to Apple's notary service through
  # scripts/release/notarize.sh, which fails unless Apple answers Accepted; the submission id, the status, the file's
  # sha256 and its cdhash go into the index entry, and the installer checks them offline (no Gatekeeper lookup).
  # Prints `, "notarization": {...}` for the spec, or nothing for a model; fails on any notarization failure.
  notary_profile="${QRTZ_NOTARY_PROFILE:-qrtz-notary}"
  notarization() { # <package dir>
    local dir="$1" kind code zipdir sub id cdh sha
    kind="$(plutil -extract kind raw -o - "$dir/qrtz.json")" || return 1
    case "$kind" in
      extension)
        code="$dir/$(plutil -extract extension.entry raw -o - "$dir/qrtz.json")" || return 1
        sub="$code"
        ;;
      service)
        sub="$dir/$(plutil -extract service.bundle raw -o - "$dir/qrtz.json")" || return 1
        code="$(find "$sub/Contents/MacOS" -type f -perm -u+x | head -1)"
        ;;
      *) return 0 ;;
    esac
    [[ -f "$code" ]] || { echo "publish: $dir: no executable to notarize ($code)" >&2; return 1; }
    zipdir="$(mktemp -d)"
    ditto -c -k --keepParent --sequesterRsrc "$sub" "$zipdir/code.zip"
    id="$(bash "$repo/scripts/release/notarize.sh" "$zipdir/code.zip" "$notary_profile" |
      sed -n 's/.*(submission \([0-9a-fA-F-]\{36\}\), Accepted).*/\1/p')" || { rm -rf "$zipdir"; return 1; }
    rm -rf "$zipdir"
    [[ -n "$id" ]] || { echo "publish: $dir: notarytool gave no Accepted submission id" >&2; return 1; }
    cdh="$(codesign -dvvv "$code" 2>&1 | sed -n 's/^CDHash=//p' | head -1)"
    [[ "$cdh" =~ ^[0-9a-f]{40}$ ]] || { echo "publish: $code: no cdhash (unsigned?)" >&2; return 1; }
    sha="$(shasum -a 256 "$code" | cut -d' ' -f1)"
    echo "publish: notarized $kind $(basename "$dir") (submission $id, cdhash $cdh)" >&2
    printf ', "notarization": { "submissionId": "%s", "status": "Accepted", "codeSha256": "%s", "cdhash": "%s" }' \
      "$(tr 'A-F' 'a-f' <<<"$id")" "$sha" "$cdh"
  }

  spec="$(mktemp -d)/publish.json"
  {
    printf '{\n  "teamId": "%s",\n  "pinnedModels": [\n' "$team"
    printf '    "%s",\n    "%s"\n  ],\n' "$repo/crates/qrtz-app/src/local_models.json" "$repo/crates/qrtz-app/src/ctrl_laya_model.json"
    printf '  "models": ['
    sep=""
    for n in $wanted; do
      d="$(model_dir "$n")"
      [[ -n "$d" ]] || die "QRTZ_PUBLISH_MODELS: $n is not a pinned model"
      if [[ -d "$models_dir/$d" ]]; then
        md="$models_dir/$d"
      else
        md="$(fetch_model "$n" "$d")" || die "$n: couldn't download it from Hugging Face to hash it"
      fi
      printf '%s\n    { "name": "%s", "dir": "%s" }' "$sep" "$n" "$md"
      sep=","
    done
    printf '\n  ],\n  "packages": ['
    sep=""
    for p in ${QRTZ_PUBLISH_PACKAGES:-}; do
      [[ -f "$p/qrtz.json" ]] || die "$p: not a built package dir (no qrtz.json)"
      n="$(notarization "$(cd "$p" && pwd)")" || die "$p: not notarized; nothing was added to the index"
      printf '%s\n    { "dir": "%s"%s }' "$sep" "$(cd "$p" && pwd)" "$n"
      sep=","
    done
    printf '\n  ]\n}\n'
  } >"$spec"
  echo "publish: spec $spec"
  echo "publish: models:${wanted:- none}; packages: ${QRTZ_PUBLISH_PACKAGES:-none}"
  "$bin" build --out "$out" --spec "$spec" --keys "$keys"
fi
"$bin" verify --out "$out" --root-sha256 "$pin"

primary="${QRTZ_PRIMARY_URL:-https://modules.qrtz.ai}"

if [[ "$out_only" == 1 ]]; then
  # No upload, no rclone, no R2 token: the list of what to upload, in order (ADR 0171).
  list="$out.upload.tsv"
  ts_only=()
  [[ "$mode" == timestamp ]] && ts_only=(--timestamp-only)
  : >"$list"
  new=0
  bytes=0
  while IFS=$'\t' read -r key size cc; do
    state=new
    if [[ "$cc" == *immutable* ]] && curl -fsSI -o /dev/null --max-time 10 "$primary/$key" 2>/dev/null; then
      state=live
    else
      new=$((new + 1))
      bytes=$((bytes + size))
    fi
    printf '%s\t%s\t%s\t%s\n' "$key" "$size" "$cc" "$state" >>"$list"
  done < <("$bin" upload-list --out "$out" "${ts_only[@]+"${ts_only[@]}"}")
  echo "publish: built and verified in $out (not uploaded)"
  echo "publish: upload list $list: $new new object(s), $bytes bytes; upload them in order from $out/<key>, the"
  echo "publish:   timestamp pair last (Cache-Control per line), then check $primary/v1/timestamp.json"
  exit 0
fi

command -v rclone >/dev/null || die "rclone not found (brew install rclone)"
for v in R2_ACCOUNT_ID R2_ACCESS_KEY_ID R2_SECRET_ACCESS_KEY; do
  [[ -n "${!v:-}" ]] || die "$v is not set"
done
# rclone remotes from the environment only (no config file with secrets).
export RCLONE_CONFIG_QRTZR2_TYPE=s3 RCLONE_CONFIG_QRTZR2_PROVIDER=Cloudflare
export RCLONE_CONFIG_QRTZR2_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID" RCLONE_CONFIG_QRTZR2_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY"
export RCLONE_CONFIG_QRTZR2_ENDPOINT="https://$R2_ACCOUNT_ID.r2.cloudflarestorage.com"
export RCLONE_CONFIG_QRTZR2_NO_CHECK_BUCKET=true RCLONE_CONFIG_QRTZR2_ACL=private

# ADR 0171: out never holds model files (qrtz-publish keeps them in $out.model-blobs for offline packs, and verify
# refuses an out that has one); only v1/ is uploaded.
upload() { # <remote:bucket>
  if [[ "$mode" != timestamp ]]; then
    # Everything but the timestamp: immutable, cached for a year.
    rclone copy "$out/v1" "$1/v1" --immutable --exclude "timestamp.json*" \
      --header-upload "Cache-Control: public, max-age=31536000, immutable" --transfers 4 --s3-chunk-size 64M -v
  fi
  # The timestamp last (its .sig first is fine: the client fetches the pair), cached 5 minutes.
  rclone copy "$out/v1" "$1/v1" --include "timestamp.json*" --header-upload "Cache-Control: public, max-age=300" --ignore-times -v
}
upload "qrtzr2:$r2_bucket"

# The live check: what the host serves is what was signed.
want="$(shasum -a 256 "$out/v1/timestamp.json" 2>/dev/null | cut -d' ' -f1 || sha256sum "$out/v1/timestamp.json" | cut -d' ' -f1)"
got="$(curl -fsS -H 'Cache-Control: no-cache' "$primary/v1/timestamp.json?nocache=$RANDOM" | (shasum -a 256 2>/dev/null || sha256sum) | cut -d' ' -f1 || true)"
if [[ "$got" == "$want" ]]; then
  echo "publish: $primary serves the new timestamp"
else
  echo "publish: WARNING: $primary doesn't serve the new timestamp yet (cache or DNS); check $primary/v1/timestamp.json in a few minutes" >&2
fi
echo "publish: done"
