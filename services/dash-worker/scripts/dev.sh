#!/usr/bin/env bash
# bun run --cwd services/dash-worker dev:local [-- --wipe]
# Starts the Agent Dash worker under `wrangler dev --local` on http://127.0.0.1:8799 (loopback only, local Durable
# Objects in services/dash-worker/.wrangler/state, no Cloudflare account, never deploys). On first run it writes
# services/dash-worker/.dev.vars (gitignored, mode 600) with throwaway DASH_PEPPER / DASH_SETUP_TOKEN and
# DASH_DEV_LOCAL=1; the pepper must stay with the state, so later runs reuse it. --wipe deletes both first.
# Copy the setup token for the app with:  bun run --cwd services/dash-worker token   (to the clipboard; never printed)
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
wipe=0
for a in "$@"; do case "$a" in --wipe) wipe=1 ;; *) die "unknown flag $a" ;; esac; done
need_node22; worker_deps
vars="$worker/.dev.vars" state="$worker/.wrangler/state"
if [[ $wipe == 1 ]]; then rm -rf "$state" "$vars"; say "wiped local dash state and .dev.vars"; fi
if [[ ! -f "$vars" ]]; then write_vars "$vars" 8799; rm -rf "$state"; say "wrote fresh local secrets to services/dash-worker/.dev.vars (not printed)"; fi
grep -q '^DASH_DEV_LOCAL=1$' "$vars" || die ".dev.vars has no DASH_DEV_LOCAL=1 (rerun with --wipe)"
# QRT-298: older .dev.vars files predate DASH_DEV_ORIGIN; add it (no secret in it).
grep -q '^DASH_DEV_ORIGIN=' "$vars" || echo "DASH_DEV_ORIGIN=http://127.0.0.1:8799" >>"$vars"
say "dash server: http://127.0.0.1:8799  (Ctrl-C to stop; test codes: POST /v1/owner/test-codes gives TST-)"
run_wrangler 8799 "$state"
