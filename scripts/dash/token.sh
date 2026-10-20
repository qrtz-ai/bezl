#!/usr/bin/env bash
# bun run --cwd services/dash-worker token: copies the local server's one-time setup token to the clipboard (macOS pbcopy), never prints it.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
vars="$worker/.dev.vars"
[[ -f "$vars" ]] || die "no services/dash-worker/.dev.vars yet: run bun run --cwd services/dash-worker dev:local first"
command -v pbcopy >/dev/null || die "pbcopy not found (macOS only)"
sed -n 's/^DASH_SETUP_TOKEN=//p' "$vars" | tr -d '\n' | pbcopy
say "setup token copied to the clipboard. Paste it in the dev app: Dash > settings > server > setup token."
