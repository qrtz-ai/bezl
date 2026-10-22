#!/bin/sh
# The echo dictation bubble on its own (hold fn to open, let go to close). macOS only; dev only, never bundled.
# Usage: native/qrtz-helper/scripts/bubble-lab.sh [--idle] [--processing]
set -eu
cd "$(dirname "$0")/.."
exec swift run qrtz-bubble-lab "$@"
