#!/usr/bin/env bash
# Typecheck QrtzCtrl's macOS-only sources on Linux against hand-written stubs of the Apple APIs (run 10). The real
# check is a macOS `swift build` of qrtz-helper; this catches language-level errors in between Mac runs.
# Usage: scripts/ctrl/mac-typecheck.sh   (needs swiftc on PATH)
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
src="$root/native/qrtz-helper/Sources/QrtzCtrl"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
"$root/scripts/ctrl/objc-names.sh"
for f in "$src"/mac/*.swift; do
  grep -v -E '^#if os\(macOS\)|^#endif|^import (AppKit|ApplicationServices|CoreML|CoreServices|Carbon|CoreGraphics|IOKit|CoreWLAN|CoreAudio)' "$f" >"$tmp/mac_$(basename "$f")"
done
# QrtzCtrl imports QrtzShared (CtrlMarker, SecureField): build its module first (Foundation-only, as in Package.swift).
mkdir -p "$tmp/mods"
swiftc -emit-module -parse-as-library -swift-version 6 -module-name QrtzShared -emit-module-path "$tmp/mods/QrtzShared.swiftmodule" \
  "$root"/native/qrtz-helper/Sources/QrtzShared/*.swift
swiftc -typecheck -swift-version 5 -I "$tmp/mods" -module-name QrtzCtrl "$root/scripts/ctrl/mac-stubs.swift" "$src"/*.swift "$tmp"/mac_*.swift
echo "QrtzCtrl mac sources typecheck against the stubs"
