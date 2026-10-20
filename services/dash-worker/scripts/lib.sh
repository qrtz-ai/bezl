# Shared bits for services/dash-worker/scripts/*.sh (docs/dash-local.md). Local only: never wrangler deploy, no Cloudflare account.
set -euo pipefail
worker="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
repo="$(git -C "$worker" rev-parse --show-toplevel)"
say() { echo "[dash] $*" >&2; }
die() { say "error: $*"; exit 1; }
need_node22() {
  command -v node >/dev/null || die "node 22+ is needed (wrangler 4.151 refuses older)"
  local major; major="$(node -p 'process.versions.node.split(".")[0]')"
  [[ "$major" -ge 22 ]] || die "node $(node -v) found; wrangler needs 22+ (e.g. brew install node@22, then put it first on PATH)"
}
worker_deps() { [[ -d "$worker/node_modules/wrangler" ]] || (cd "$worker" && bun install); }
# A fresh local-only secrets file (mode 600). Values are never printed. DASH_DEV_LOCAL=1 turns on the worker's local
# test mode (src/dev.ts): http from 127.0.0.1 and TST- codes. This file is the only place that variable exists.
# DASH_ENROLL_KEY (QRT-284) is a throwaway local key for the enrollment-code HMAC; production sets its own secret.
write_vars() {
  local f="$1"
  ( umask 077
    { echo "DASH_PEPPER=$(openssl rand -hex 32)"
      echo "DASH_ENROLL_KEY=$(openssl rand -hex 32)"
      echo "DASH_SETUP_TOKEN=$(openssl rand -hex 24)"
      echo "DASH_DEV_LOCAL=1"; } >"$f" )
  [[ "$(sed -n 's/^DASH_PEPPER=//p' "$f" | tr -d '\n' | wc -c | tr -d ' ')" == 64 ]] || die "bad pepper written"
  [[ "$(sed -n 's/^DASH_ENROLL_KEY=//p' "$f" | tr -d '\n' | wc -c | tr -d ' ')" == 64 ]] || die "bad enroll key written"
}
# wrangler dev, local simulation only, listening on 127.0.0.1 only.
run_wrangler() { # port persist-dir [extra args]
  local port="$1" persist="$2"; shift 2
  # --local-upstream/--upstream-protocol: keep the request URL http://127.0.0.1:<port> (not the dash.qrtz.ai route).
  cd "$worker" && exec npx --no-install wrangler dev --local --ip 127.0.0.1 --port "$port" --persist-to "$persist" \
    --local-upstream "127.0.0.1:$port" --upstream-protocol http \
    --show-interactive-dev-session=false "$@"
}
# QRT-284: an older .dev.vars has no DASH_ENROLL_KEY; add a throwaway one (outstanding local codes stop working).
add_enroll_key() {
  local f="$1"
  grep -q '^DASH_ENROLL_KEY=' "$f" && return 0
  ( umask 077; echo "DASH_ENROLL_KEY=$(openssl rand -hex 32)" >>"$f" )
  say "added a local DASH_ENROLL_KEY to services/dash-worker/.dev.vars (not printed)"
}
