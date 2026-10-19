#!/usr/bin/env bash
# Run qrtz entirely from this checkout (debug only): nano, snap, ctrl and echo from native/qrtz-helper/.build/debug,
# the app from target/debug, against an isolated data dir. See docs/dev/dev-local.md.
#
#   scripts/dev/dev-local.sh [--without <id>]... [--quit-installed] [--build-only]
#   bun run dev:local [-- <flags>]
#
#   --without <id>     leave nano, snap, ctrl or echo out (it then isn't installed in the dev data dir). Repeatable.
#   --quit-installed   quit /Applications/qrtz.app first (both would grab fn). Without it the installed app is never
#                      touched; the script only warns.
#   --build-only       build and print the environment, don't launch.
#
# Never touches /Applications/qrtz.app (except --quit-installed asking it to quit), the real data dir
# (~/Library/Application Support/ai.qrtz.desktop), R2, the keychain or any update key. ctrl, like on a real install,
# still asks for its consent and has to be switched on in the dev app (ADR 0216).
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
say() { echo "[dev-local] $*" >&2; }
die() { say "error: $*"; exit 1; }

ids=(nano snap ctrl echo)
without=() quit_installed=0 build_only=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --without) [[ $# -ge 2 ]] || die "--without needs an id"; without+=("$2"); shift 2 ;;
    --quit-installed) quit_installed=1; shift ;;
    --build-only) build_only=1; shift ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) die "unknown flag $1" ;;
  esac
done
for w in "${without[@]}"; do [[ " ${ids[*]} " == *" $w "* ]] || die "--without $w: not one of ${ids[*]}"; done
[[ "$(uname -s)" == Darwin ]] || die "macOS only"

real="$HOME/Library/Application Support/ai.qrtz.desktop"
export QRTZ_USER_DATA="${QRTZ_DEV_LOCAL_DATA:-$HOME/Library/Application Support/ai.qrtz.desktop-dev}"
[[ "$QRTZ_USER_DATA" == /* ]] || die "QRTZ_USER_DATA must be absolute"
mkdir -p "$QRTZ_USER_DATA"; chmod 700 "$QRTZ_USER_DATA"
ud="$(cd "$QRTZ_USER_DATA" && pwd -P)"; rr="$( [[ -d "$real" ]] && cd "$real" && pwd -P || echo "$real")"
[[ "$ud" == "$rr" || "$ud" == "$rr"/* ]] && die "QRTZ_USER_DATA ($ud) is the real data dir or inside it: refused"

# 1. Build: the Swift module helpers, then the app with the dev-local feature (debug only, never --release).
say "swift build (native/qrtz-helper: qrtz-nano qrtz-snap qrtz-ctrl qrtz-echo)"
(cd "$repo/native/qrtz-helper" && for id in "${ids[@]}"; do swift build -c debug --product "qrtz-$id"; done)
bin="$(cd "$repo/native/qrtz-helper" && swift build -c debug --show-bin-path)"
say "cargo build -p qrtz-desktop --features dev-local (debug)"
(cd "$repo" && cargo build -p qrtz-desktop --features dev-local)

# 2. Environment: the dev data dir and the local products. The app checks every path itself (src/dev_local.rs).
export QRTZ_DEV_REPO="$repo"
for id in "${ids[@]}"; do
  var="QRTZ_$(tr '[:lower:]' '[:upper:]' <<<"$id")_BIN"
  if [[ " ${without[*]} " == *" $id "* ]]; then unset "$var"; say "$id: left out (--without)"; continue; fi
  [[ -x "$bin/qrtz-$id" ]] || die "$bin/qrtz-$id wasn't built"
  export "$var=$bin/qrtz-$id"; say "$id: $bin/qrtz-$id"
done
say "data dir: $QRTZ_USER_DATA (the real one is never read)"

cat >&2 <<'NOTE'
[dev-local] ─────────────────────────────────────────────────────────────────────────────────────────────
[dev-local] HEADS UP: the dev app is a DIFFERENT app to macOS (signed ai.qrtz.desktop.dev, run from target/debug).
[dev-local] It has none of qrtz's grants: give it its own Accessibility and Microphone grant in System Settings ›
[dev-local] Privacy & Security when it asks. Running it next to (or instead of) /Applications/qrtz.app breaks echo's
[dev-local] fn hotkey for the installed app: both grab fn. Quit the dev app to get your normal dictation back.
[dev-local] ─────────────────────────────────────────────────────────────────────────────────────────────
NOTE

if pgrep -f '/Applications/qrtz.app/Contents/MacOS/' >/dev/null 2>&1; then
  if [[ $quit_installed == 1 ]]; then
    say "quitting /Applications/qrtz.app (--quit-installed); reopen it when you're done"
    osascript -e 'tell application id "ai.qrtz.desktop" to quit' || true
  else
    say "WARNING: /Applications/qrtz.app is running. Both apps will fight over fn and dictation will break."
    say "         Quit it yourself or rerun with --quit-installed. It is not touched otherwise."
  fi
fi

[[ $build_only == 1 ]] && { say "built (--build-only): not launching"; exit 0; }
say "launching (tauri dev --features dev-local); it logs 'dev-local: <id> from <path>' and shows a dev-local badge"
cd "$repo/apps/desktop" && exec bun --bun run tauri dev --features dev-local
