#!/usr/bin/env bash
# QRT-297: the package host is the fixed `index` GitHub release of qrtz-ai/qrtz (it replaced Cloudflare R2). A
# release holds flat asset names, so layout path `v1/a/b` is the asset `v1.a.b` (qrtz_modules::index::layout::on_host)
# and the app fetches https://github.com/qrtz-ai/qrtz/releases/download/index/v1.a.b (never api.github.com).
#
#   scripts/release/gh-store.sh upload <out dir> [--timestamp-only]
#       every object of <out>/v1 (never v1/app/, never the model blob dir) not on the release yet, then the timestamp
#       pair last (--clobber: the only assets ever replaced). Content-addressed objects are immutable: an asset that
#       is already up is skipped, never replaced (scripts/ci/verify-live.sh then checks every byte).
#   scripts/release/gh-store.sh hydrate <out dir>
#       the reverse: every module asset of the release back into <out>/v1/… (qrtz-publish continues from it).
#   scripts/release/gh-store.sh upload-app <dir with qrtz_<v>_aarch64.app.tar.gz{,.sig} and latest.json> <dmg>
#       qrtz.app's update (VERSION=<v>): the versioned archive, its .sig and the dmg, then v1.app.qrtz-latest.dmg and
#       last v1.app.latest.json, so no feed ever names an archive that isn't up yet.
#   scripts/release/gh-store.sh upload-feed <latest.json>
#       only v1.app.latest.json (the weekly feed re-sign, publish-local.sh --refresh / release.yml refresh).
#   scripts/release/gh-store.sh asset <layout path>     print the asset name (tests)
#
# Needs gh, authenticated by GH_TOKEN (in Actions: the release job's GITHUB_TOKEN, contents: write). Env: GH_REPO
# (qrtz-ai/qrtz), QRTZ_STORE_TAG (index). No other credential: nothing here touches Cloudflare.
set -euo pipefail
die() { echo "gh-store: $*" >&2; exit 1; }
repo_slug="${GH_REPO:-qrtz-ai/qrtz}"
tag="${QRTZ_STORE_TAG:-index}"
export GH_REPO="$repo_slug"

asset() { printf '%s' "${1//\//.}"; }
# The layout path of an asset name, or nothing for a name that isn't a module object (the app's files, strays).
path_of() {
  local n="$1" h='[0-9a-f]{64}'
  if [[ "$n" =~ ^v1\.timestamp\.json(\.sig)?$ ]]; then echo "v1/timestamp.json${BASH_REMATCH[1]}"
  elif [[ "$n" =~ ^v1\.root\.([1-9][0-9]*)\.root\.json(\.sig)?$ ]]; then echo "v1/root/${BASH_REMATCH[1]}.root.json${BASH_REMATCH[2]}"
  elif [[ "$n" =~ ^v1\.index\.($h)\.json(\.sig)?$ ]]; then echo "v1/index/${BASH_REMATCH[1]}.json${BASH_REMATCH[2]}"
  elif [[ "$n" =~ ^v1\.blobs\.sha256\.($h)$ ]]; then echo "v1/blobs/sha256/${BASH_REMATCH[1]}"
  fi
}

ensure_release() {
  gh release view "$tag" >/dev/null 2>&1 && return 0
  echo "gh-store: creating the $tag release on $repo_slug"
  gh release create "$tag" --title "qrtz package index" --latest=false \
    --notes "The signed qrtz package index (QRT-297). Machine-read by qrtz; nothing here to download by hand." >/dev/null
}
existing() { gh release view "$tag" --json assets --jq '.assets[].name'; }
put() { # <file> <asset name> [--clobber]
  local stage
  stage="$(mktemp -d)"
  cp "$1" "$stage/$2"
  gh release upload "$tag" "$stage/$2" ${3:+"$3"} >/dev/null
  rm -rf "$stage"
  echo "gh-store: up $2"
}

cmd="${1:-}"
case "$cmd" in
  asset) asset "${2:?asset <layout path>}"; echo ;;
  upload)
    out="${2:?upload <out dir> [--timestamp-only]}"
    [[ -f "$out/v1/timestamp.json" && -f "$out/v1/timestamp.json.sig" ]] || die "$out/v1: no signed timestamp"
    ensure_release
    have="$(existing)"
    if [[ "${3:-}" != --timestamp-only ]]; then
      n=0
      while IFS= read -r f; do
        key="${f#"$out"/}"
        case "$key" in v1/app/* | v1/timestamp.json*) continue ;; esac
        [[ -n "$(path_of "$(asset "$key")")" ]] || die "$key: not a layout path"
        name="$(asset "$key")"
        grep -qxF "$name" <<<"$have" && continue
        put "$f" "$name"
        n=$((n + 1))
      done < <(find "$out/v1" -type f | LC_ALL=C sort)
      echo "gh-store: $n new object(s)"
    fi
    put "$out/v1/timestamp.json.sig" v1.timestamp.json.sig --clobber
    put "$out/v1/timestamp.json" v1.timestamp.json --clobber
    ;;
  hydrate)
    out="${2:?hydrate <out dir>}"
    dl="$(mktemp -d)"
    trap 'rm -rf "$dl"' EXIT
    mkdir -p "$out/v1"
    if ! gh release view "$tag" >/dev/null 2>&1; then
      echo "gh-store: no $tag release yet (first publish): $out stays empty"
      exit 0
    fi
    n=0
    while IFS= read -r name; do
      p="$(path_of "$name")"
      [[ -n "$p" ]] || continue
      [[ -f "$out/$p" ]] && continue
      gh release download "$tag" --pattern "$name" --dir "$dl" --clobber
      mkdir -p "$(dirname "$out/$p")"
      mv "$dl/$name" "$out/$p"
      n=$((n + 1))
    done < <(existing)
    echo "gh-store: $n object(s) into $out/v1"
    ;;
  upload-app)
    dir="${2:?upload-app <dir> <dmg>}"
    dmg="${3:?upload-app <dir> <dmg>}"
    v="${VERSION:?VERSION is not set}"
    [[ "$v" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "not a plain x.y.z version: $v"
    tgz="$dir/qrtz_${v}_aarch64.app.tar.gz"
    # QRT-275: every release attaches its source (scripts/release/source-archives.sh); none is optional.
    src=("qrtz_${v}_source.tar.gz" "qrtz_${v}_fluidaudio_source.tar.gz" "qrtz_${v}_build-scripts.tar.gz" "qrtz_${v}_source.sha256")
    for f in "$tgz" "$tgz.sig" "$dir/latest.json" "$dmg"; do [[ -s "$f" ]] || die "missing $f"; done
    for f in "${src[@]}"; do [[ -s "$dir/$f" ]] || die "missing $dir/$f"; done
    ensure_release
    have="$(existing)"
    for pair in "$tgz|v1/app/$v/qrtz_${v}_aarch64.app.tar.gz" "$tgz.sig|v1/app/$v/qrtz_${v}_aarch64.app.tar.gz.sig" \
      "$dmg|v1/app/$v/qrtz_${v}_aarch64.dmg" "$dir/${src[0]}|v1/app/$v/${src[0]}" "$dir/${src[1]}|v1/app/$v/${src[1]}" \
      "$dir/${src[2]}|v1/app/$v/${src[2]}" "$dir/${src[3]}|v1/app/$v/${src[3]}"; do
      name="$(asset "${pair#*|}")"
      grep -qxF "$name" <<<"$have" && die "$name is already up (versions are immutable; bump the version)"
      put "${pair%%|*}" "$name"
    done
    put "$dmg" "$(asset v1/app/qrtz-latest.dmg)" --clobber
    put "$dir/latest.json" "$(asset v1/app/latest.json)" --clobber
    echo "gh-store: qrtz $v is up"
    ;;
  upload-feed)
    f="${2:?upload-feed <latest.json>}"
    [[ -s "$f" ]] || die "missing $f"
    ensure_release
    put "$f" "$(asset v1/app/latest.json)" --clobber
    ;;
  *) sed -n '2,24p' "$0"; exit 2 ;;
esac
