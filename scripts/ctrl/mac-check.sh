#!/usr/bin/env bash
# ctrl's macOS build + read-only Mac check (run 15).
#   scripts/ctrl/mac-check.sh --typecheck   typecheck the check on Linux against the stubs (needs swiftc)
#   scripts/ctrl/mac-check.sh OUT.tgz        bundle native/qrtz-helper (as committed at HEAD) + the check for the Mac
# The bundle carries FluidAudio (the pinned tag) and its NemoTextProcessing.xcframework zip, fetched here on the box and
# checked against FluidAudio's own manifest checksum, so the Mac build downloads nothing (run 24 stalled 21 min on that
# artifact download; ADR 0142). The bundled helper manifest points at ../FluidAudio by path; the repo's is unchanged.
# On the Mac: mkdir D && tar xzf OUT.tgz -C D && bash D/run.sh; rm -rf D. See mac-check/main.swift for what it reads.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
src="$root/native/qrtz-helper/Sources/QrtzCtrl"
here="$root/scripts/ctrl/mac-check"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
if [ "${1:-}" = "--typecheck" ]; then
  strip() { grep -v -E '^#if os\(macOS\)|^#endif|^import (AppKit|ApplicationServices|CoreML|CoreServices|Carbon|CoreGraphics|IOKit|CoreWLAN)' "$1"; }
  for f in "$src"/mac/*.swift; do strip "$f" >"$tmp/mac_$(basename "$f")"; done
  strip "$here/main.swift" >"$tmp/main.swift"
  swiftc -typecheck -swift-version 5 -module-name ctrl_check "$root/scripts/ctrl/mac-stubs.swift" "$here/stubs-extra.swift" \
    "$src"/*.swift "$tmp"/mac_*.swift "$tmp/main.swift"
  echo "ctrl-check typechecks against the stubs"
  exit 0
fi
out="${1:?usage: mac-check.sh OUT.tgz | --typecheck}"
mkdir -p "$tmp/b/ctrl-check/Sources/ctrl-check"
git -C "$root" archive HEAD native/qrtz-helper | tar x -C "$tmp"
mv "$tmp/native/qrtz-helper" "$tmp/b/qrtz-helper"
cp -R "$src"/. "$tmp/b/ctrl-check/Sources/ctrl-check/"
cp "$here/main.swift" "$tmp/b/ctrl-check/Sources/ctrl-check/main.swift"
cp "$here/run.sh" "$tmp/b/run.sh"
fa_tag="${QRTZ_FLUIDAUDIO_TAG:-v0.17.7}"
cache="${XDG_CACHE_HOME:-$HOME/.cache}/qrtz-ctrl-mac-check"
mkdir -p "$cache"
if [ ! -d "$cache/FluidAudio-$fa_tag/.git" ]; then
  rm -rf "$cache/FluidAudio-$fa_tag"
  git -c advice.detachedHead=false clone -q --depth 1 --branch "$fa_tag" https://github.com/FluidInference/FluidAudio.git "$cache/FluidAudio-$fa_tag"
fi
mkdir -p "$tmp/b/FluidAudio"
git -C "$cache/FluidAudio-$fa_tag" archive HEAD | tar x -C "$tmp/b/FluidAudio"
url="$(grep -h -o 'https://[^"]*NemoTextProcessing.xcframework.zip' "$tmp/b/FluidAudio"/Package*.swift | sort -u)"
sum="$(grep -h -A3 'NemoTextProcessing.xcframework.zip' "$tmp/b/FluidAudio"/Package*.swift | grep -o '[0-9a-f]\{64\}' | sort -u)"
[ "$(echo "$url" | wc -l)" = 1 ] && [ "$(echo "$sum" | wc -l)" = 1 ] || { echo "FluidAudio $fa_tag: not one artifact url/checksum" >&2; exit 1; }
zip="$cache/$(basename "$(dirname "$url")")-NemoTextProcessing.xcframework.zip"
if [ ! -f "$zip" ]; then curl -fsSL -o "$zip.part" "$url" && mv "$zip.part" "$zip"; fi
[ "$(sha256sum "$zip" | cut -d' ' -f1)" = "$sum" ] || { echo "artifact checksum mismatch: $zip" >&2; rm -f "$zip"; exit 1; }
cp "$zip" "$tmp/b/FluidAudio/NemoTextProcessing.xcframework.zip"
# url+checksum -> a local path (run.sh unzips it on the Mac with ditto, keeping the framework layout).
python3 - "$tmp/b/FluidAudio" <<'PY'
import re, sys, pathlib
for m in pathlib.Path(sys.argv[1]).glob("Package*.swift"):
    s = m.read_text()
    s2, n = re.subn(r'url:\s*"https://[^"]*NemoTextProcessing\.xcframework\.zip",\s*checksum:\s*"[0-9a-f]{64}"',
                    'path: "NemoTextProcessing.xcframework"', s)
    if n != 1:
        sys.exit(f"{m.name}: expected one NemoTextProcessing binaryTarget, got {n}")
    m.write_text(s2)
PY
pkg="$tmp/b/qrtz-helper/Package.swift"
grep -q '\.package(url: "https://github.com/FluidInference/FluidAudio.git"' "$pkg" || { echo "helper manifest: FluidAudio dep not found" >&2; exit 1; }
sed -i -E 's#\.package\(url: "https://github.com/FluidInference/FluidAudio.git", [^)]*\)\)#.package(path: "../FluidAudio")#' "$pkg"
grep -q '\.package(path: "../FluidAudio")' "$pkg" || { echo "helper manifest: rewrite failed" >&2; exit 1; }
rm -f "$tmp/b/qrtz-helper/Package.resolved"
cat >"$tmp/b/ctrl-check/Package.swift" <<'PKG'
// swift-tools-version: 5.9
import PackageDescription
let package = Package(name: "ctrl-check", platforms: [.macOS(.v14)],
                      targets: [.executableTarget(name: "ctrl-check", path: "Sources/ctrl-check")])
PKG
tar czf "$out" -C "$tmp/b" .
echo "wrote $out"
