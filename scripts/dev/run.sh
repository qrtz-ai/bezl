#!/usr/bin/env bash
# Cargo runner for apps/desktop/src-tauri on macOS (.cargo/config.toml), so `tauri dev` / `bun start` runs a
# TCC-stable qrtz-app: scripts/dev/sign.sh gives it and its helpers stable signatures, and scripts/dev/launch.c makes
# it its own responsible process (so the grant macOS checks is qrtz-app's, not your terminal's). Anything that isn't
# qrtz-app (tests, the `qrtz` command) runs untouched. QRTZ_DEV_NO_SIGN=1 turns it all off. See docs/dev-permissions.md.
set -euo pipefail
bin="$1"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ "$(uname -s)" != Darwin || "$(basename "$bin")" != qrtz-app || "${QRTZ_DEV_NO_SIGN:-0}" == 1 ]]; then
  exec "$@"
fi
dir="$(cd "$(dirname "$bin")" && pwd)"
"$here/sign.sh" "$dir" || echo "[qrtz dev] signing failed; privacy grants won't stick for this run" >&2
launcher="$dir/.qrtz-dev-launch"
if [[ ! -x "$launcher" || "$here/launch.c" -nt "$launcher" ]]; then
  cc -O2 -o "$launcher.tmp" "$here/launch.c" 2>/dev/null && mv "$launcher.tmp" "$launcher" \
    || echo "[qrtz dev] couldn't build the launcher (xcode-select --install); your terminal owns the grants" >&2
fi
[[ -x "$launcher" ]] && exec "$launcher" "$@"
exec "$@"
