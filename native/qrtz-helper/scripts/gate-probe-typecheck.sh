#!/usr/bin/env bash
# ADR 0146 M1.3: typecheck the module apps' `--qrtz-gate-probe` macOS code (Sources/QrtzGateProbe) on Linux against
# the gate probe's stubs (scripts/modules/probe-stubs.swift, the scripts/modules/probe-typecheck.sh pattern):
# `#if os(macOS)` becomes a -D flag and the Apple imports are dropped. A clean run catches language mistakes, not
# SDK drift; building the module apps on the Mac is the real check.
# Usage: native/qrtz-helper/scripts/gate-probe-typecheck.sh   (needs swiftc on PATH)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
src="$here/../Sources/QrtzGateProbe"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
for f in "$src/GateProbe.swift" "$src/mac/MacGateProbeBackend.swift"; do
  sed -E -e 's/^#if os\(macOS\)/#if QRTZ_PROBE_MAC/' -e '/^import (ApplicationServices|AVFoundation|CoreAudio|CoreGraphics|CoreServices)$/d' \
    "$f" >"$tmp/$(basename "$f")"
done
swiftc -typecheck -swift-version 5 -D QRTZ_PROBE_MAC "$repo/scripts/modules/probe-stubs.swift" "$tmp"/*.swift
echo "QrtzGateProbe typechecks (mac branch against the stubs)"
