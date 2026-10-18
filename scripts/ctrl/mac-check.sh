#!/usr/bin/env bash
# ctrl's macOS build + read-only Mac check (run 15).
#   scripts/ctrl/mac-check.sh --typecheck   typecheck the check on Linux against the stubs (needs swiftc)
#   scripts/ctrl/mac-check.sh OUT.tgz        bundle native/qrtz-helper (as committed at HEAD) + the check for the Mac
# On the Mac: mkdir D && tar xzf OUT.tgz -C D && bash D/run.sh; rm -rf D. See mac-check/main.swift for what it reads.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
src="$root/native/qrtz-helper/Sources/QrtzCtrl"
here="$root/scripts/ctrl/mac-check"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
if [ "${1:-}" = "--typecheck" ]; then
  strip() { grep -v -E '^#if os\(macOS\)|^#endif|^import (AppKit|ApplicationServices|CoreML|CoreServices|Carbon|CoreGraphics)' "$1"; }
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
cat >"$tmp/b/ctrl-check/Package.swift" <<'PKG'
// swift-tools-version: 5.9
import PackageDescription
let package = Package(name: "ctrl-check", platforms: [.macOS(.v14)],
                      targets: [.executableTarget(name: "ctrl-check", path: "Sources/ctrl-check")])
PKG
tar czf "$out" -C "$tmp/b" .
echo "wrote $out"
