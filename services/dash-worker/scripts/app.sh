#!/usr/bin/env bash
# bun run --cwd services/dash-worker app
# Runs the dev app (debug, dev-local feature, identifier ai.qrtz.desktop.dev, its own data dir) with Agent Dash
# shown and its helper pointed at the local server from `bun run --cwd services/dash-worker dev:local`:
#   QRTZ_DASH_ORIGIN=http://127.0.0.1:8799 (honoured only by debug + dev-local builds; release has no such path)
#   qrtz-dash-net built with --features dev-local next to the app; Keychain item service: qrtz-dash-dev.
# Never touches /Applications/qrtz.app, its data dir or the real ai.qrtz.dash-net Keychain item. macOS only.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
[[ "$(uname -s)" == Darwin ]] || die "macOS only"
curl -fsS -m 3 http://127.0.0.1:8799/v1/readme >/dev/null || die "no local dash server on 127.0.0.1:8799: run bun run --cwd services/dash-worker dev:local first"
export QRTZ_DASH_ORIGIN="http://127.0.0.1:8799"
export QRTZ_USER_DATA="${QRTZ_DASH_DEV_DATA:-$HOME/Library/Application Support/ai.qrtz.desktop-dash-dev}"
mkdir -p "$QRTZ_USER_DATA"; chmod 700 "$QRTZ_USER_DATA"
say "cargo build -p qrtz-dash-net --features dev-local (debug)"
(cd "$repo" && cargo build -p qrtz-dash-net --features dev-local)
say "data dir: $QRTZ_USER_DATA; helper origin: $QRTZ_DASH_ORIGIN"
cd "$repo/apps/desktop" && exec bun --bun run tauri dev --features dev-local \
  --config '{"identifier":"ai.qrtz.desktop.dev","build":{"devUrl":"http://localhost:1420/?dev=1"}}'
