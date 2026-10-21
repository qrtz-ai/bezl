#!/usr/bin/env bash
# QRT-275: the source every qrtz release attaches (AGPL-3.0 section 6, GPL-3.0 section 6 for the possibly
# espeak-derived parts of FluidAudio). scripts/app/publish-local.sh and release.yml run it and
# scripts/release/gh-store.sh upload-app refuses a release without its output; it goes up as the index release's
# v1.app.<v>.* assets next to the dmg, and release.yml also attaches it to the app-v<v> release (QRT-297).
#
#   scripts/release/source-archives.sh <version> <out dir> [<commit>]   (commit: default HEAD, the one tagged app-v<v>)
#
# Writes into <out dir>:
#   qrtz_<v>_source.tar.gz          git archive of the release commit (the app-v<v> tag source)
#   qrtz_<v>_fluidaudio_source.tar.gz  FluidAudio at exactly the version and revision native/qrtz-helper/Package.resolved
#                                   pins (fetched by revision and checked; never a branch or a newer tag)
#   qrtz_<v>_build-scripts.tar.gz   the build and release scripts and the manifests/lockfiles they read
#   qrtz_<v>_source.sha256          sha256 of the three, one per line (gh-store.sh upload-app uploads exactly these)
# QRTZ_FLUIDAUDIO_GIT overrides the FluidAudio remote (tests use a local repo). Reads no key or secret.
set -euo pipefail
v="${1:?usage: source-archives.sh <version> <out dir> [<commit>]}"
out="${2:?usage: source-archives.sh <version> <out dir> [<commit>]}"
ref="${3:-HEAD}"
[[ "$v" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "source-archives: not a plain x.y.z version: $v" >&2; exit 1; }
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
die() { echo "source-archives: $*" >&2; exit 1; }
commit="$(git -C "$repo" rev-parse --verify "$ref^{commit}")" || die "no commit $ref"
mkdir -p "$out"
sha() { if command -v shasum >/dev/null; then shasum -a 256 "$1"; else sha256sum "$1"; fi | cut -d' ' -f1; }

# 1. qrtz itself, exactly the release commit.
src="$out/qrtz_${v}_source.tar.gz"
git -C "$repo" archive --format=tar.gz --prefix="qrtz-$v/" -o "$src" "$commit"

# 2. FluidAudio, the exact pinned revision.
resolved="$(git -C "$repo" show "$commit:native/qrtz-helper/Package.resolved")"
fa_rev="$(node -e 'const p=JSON.parse(require("fs").readFileSync(0,"utf8")).pins.find(p=>p.identity==="fluidaudio");if(!p)process.exit(1);console.log(p.state.revision+" "+p.state.version+" "+p.location)' <<<"$resolved")" \
  || die "Package.resolved pins no fluidaudio"
read -r rev fav loc <<<"$fa_rev"
[[ "$rev" =~ ^[0-9a-f]{40}$ ]] || die "FluidAudio revision isn't a full sha: $rev"
git -C "$repo" show "$commit:native/qrtz-helper/Package.swift" | grep -q "exact: \"$fav\"" \
  || die "Package.swift doesn't pin FluidAudio exact: \"$fav\""
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
git -C "$tmp" init -q fa
git -C "$tmp/fa" fetch -q --depth 1 "${QRTZ_FLUIDAUDIO_GIT:-$loc}" "$rev"
[[ "$(git -C "$tmp/fa" rev-parse FETCH_HEAD)" == "$rev" ]] || die "fetched FluidAudio isn't $rev"
fa="$out/qrtz_${v}_fluidaudio_source.tar.gz"
git -C "$tmp/fa" archive --format=tar.gz --prefix="FluidAudio-$fav/" -o "$fa" "$rev"

# 3. The build scripts and what they read.
paths=(scripts package.json bun.lock Cargo.toml Cargo.lock rust-toolchain.toml native/qrtz-helper/Package.swift
  native/qrtz-helper/Package.resolved apps/desktop/src-tauri/tauri.conf.json apps/desktop/src-tauri/build.rs docs/release.md)
have=()
for p in "${paths[@]}"; do git -C "$repo" cat-file -e "$commit:$p" 2>/dev/null && have+=("$p"); done
for p in apps/desktop/src-tauri/tauri.macos.conf.json; do git -C "$repo" cat-file -e "$commit:$p" 2>/dev/null && have+=("$p"); done
bs="$out/qrtz_${v}_build-scripts.tar.gz"
git -C "$repo" archive --format=tar.gz --prefix="qrtz-$v-build-scripts/" -o "$bs" "$commit" "${have[@]}"

sums="$out/qrtz_${v}_source.sha256"
: >"$sums"
for f in "$src" "$fa" "$bs"; do [[ -s "$f" ]] || die "empty $f"; echo "$(sha "$f")  $(basename "$f")" >>"$sums"; done
echo "source-archives: qrtz $v at $commit, FluidAudio $fav at $rev"
cat "$sums"
