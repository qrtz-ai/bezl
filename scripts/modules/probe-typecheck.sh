#!/usr/bin/env bash
# ADR 0144: typecheck the Mac gate probe's macOS-only code on Linux against probe-stubs.swift (the
# scripts/ctrl/mac-typecheck.sh pattern): `#if os(macOS)` becomes a -D flag and the Apple imports are dropped.
# Usage: scripts/modules/probe-typecheck.sh   (needs swiftc on PATH)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/mac" "$tmp/linux"
sed -E -e 's/^#if os\(macOS\)/#if QRTZ_PROBE_MAC/' -e '/^import (ApplicationServices|AVFoundation|CoreServices|XPC)$/d' \
  "$here/gate-probe.swift" >"$tmp/mac/main.swift"
printf 'let probeBuild = "typecheck"\n' >"$tmp/mac/build.swift"
swiftc -typecheck -swift-version 5 -D QRTZ_PROBE_MAC "$here/probe-stubs.swift" "$tmp/mac/main.swift" "$tmp/mac/build.swift"
# And as it builds on Linux (the non-mac branch).
printf 'let probeBuild = "typecheck"\n' >"$tmp/linux/build.swift"
cp "$here/gate-probe.swift" "$tmp/linux/main.swift"
swiftc -typecheck -swift-version 5 "$tmp/linux/main.swift" "$tmp/linux/build.swift"
echo "gate probe typechecks (mac branch against the stubs, and the Linux branch)"
