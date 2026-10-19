#!/usr/bin/env bash
# ADR 0217: typecheck nano's macOS-only EventKit store (native/qrtz-helper/Sources/QrtzNanoMac/Calendar.swift) on
# Linux against eventkit-stubs.swift (the scripts/ctrl/mac-typecheck.sh pattern): `#if os(macOS)` becomes a -D flag,
# the module imports are dropped and the sources it uses (QrtzShared, nano's Calendar.swift) join one module.
# Usage: scripts/nano/mac-typecheck.sh   (needs swiftc on PATH)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
src="$here/../../native/qrtz-helper/Sources"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
strip() { sed -E -e 's/^#if os\(macOS\)/#if QRTZ_NANO_MAC/' -e '/^import (EventKit|QrtzNano|QrtzShared)$/d' "$1"; }
strip "$src/QrtzNanoMac/Calendar.swift" >"$tmp/EventKitStore.swift"
strip "$src/QrtzNano/Calendar.swift" >"$tmp/Calendar.swift"
swiftc -typecheck -swift-version 5 -D QRTZ_NANO_MAC -module-name NanoCalendarCheck "$here/eventkit-stubs.swift" \
  "$src"/QrtzShared/*.swift "$tmp/Calendar.swift" "$tmp/EventKitStore.swift"
echo "nano EventKit store typechecks against the stubs"
