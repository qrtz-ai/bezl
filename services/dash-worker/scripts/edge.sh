#!/bin/bash
# QRT-316: Cloudflare edge settings for dash.qrtz.ai. Run on Jared's Mac only, never from the box, CI or an agent,
# and never through the Cloudflare connector. bash 3.2 compatible. Needs curl and jq.
#   bash scripts/edge.sh             dry run: shows current vs wanted, writes nothing
#   bash scripts/edge.sh --apply     applies, then reads every value back and fails on any mismatch
#   bash scripts/edge.sh --revert    removes only the QRT-316 rate-limit rule (HTTPS/TLS/HSTS stay)
#   add --token-prompt to skip wrangler's login and paste a scoped API token at a hidden prompt (recommended)
# What it sets on the qrtz.ai zone:
#   - rate-limit rule (ref qrt316-enroll) on POST dash.qrtz.ai/v1/enroll, per ip.src + cf.colo.id:
#     20 req / 60 s (block 600 s) if the zone's plan allows a 60 s period, otherwise 5 req / 10 s (block 10 s);
#     blocked requests get a JSON 429 with retry_after_ms, as the agent README expects
#   - always_use_https on, min_tls_version 1.2
#   - HSTS (security_header): max-age 15552000, no includeSubDomains, no preload, nosniff
# The token needs only Zone WAF:Edit and Zone Settings:Edit on qrtz.ai. It is never printed, logged, put on a
# command line or written to disk: curl reads the header from stdin (-K -).
set -u
set +x
ZONE_NAME="qrtz.ai"
HOST="dash.qrtz.ai"
RULE_REF="qrt316-enroll"
API="${CF_API_BASE:-https://api.cloudflare.com/client/v4}"
EXPR="(http.host eq \"$HOST\" and http.request.uri.path eq \"/v1/enroll\" and http.request.method eq \"POST\")"
HSTS_MAX_AGE=15552000

mode=dry; prompt=0
for a in "$@"; do
  case "$a" in
    --apply) mode=apply ;;
    --revert) mode=revert ;;
    --token-prompt) prompt=1 ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "[edge] error: unknown argument $a" >&2; exit 2 ;;
  esac
done

say() { echo "[edge] $*" >&2; }
die() { say "error: $*"; exit 1; }
warn() { say "WARNING: $*"; }

[ "$(uname -s)" = Darwin ] || die "run this on Jared's Mac only (never the box, CI or an agent)"
command -v curl >/dev/null || die "curl is needed"
command -v jq >/dev/null || die "jq is needed (macOS 15+ ships /usr/bin/jq; else brew install jq)"

tmp="$(mktemp -d)"; chmod 700 "$tmp"
TOKEN=""
cleanup() { TOKEN=""; rm -rf "$tmp"; }
trap cleanup EXIT

from_wrangler=0
if [ "$prompt" -eq 0 ]; then
  cd "$(dirname "$0")/.." || die "cannot cd to services/dash-worker"
  TOKEN="$(npx --no-install wrangler auth token 2>/dev/null | tail -n 1 | tr -d '[:space:]')" || TOKEN=""
  [ -n "$TOKEN" ] && from_wrangler=1
fi
if [ -z "$TOKEN" ]; then
  [ "$prompt" -eq 1 ] || say "no token from wrangler login; paste a scoped API token instead"
  say "token needs only Zone WAF:Edit + Zone Settings:Edit on $ZONE_NAME (input hidden, never printed)"
  printf 'Cloudflare API token: ' >&2
  IFS= read -rs TOKEN || TOKEN=""
  echo >&2
  [ -n "$TOKEN" ] || die "no token given"
fi

# cf METHOD PATH [json-file]: response body to stdout; returns 1 on HTTP >= 400 or success != true.
cf() {
  local m="$1" p="$2" f="${3:-}" out code
  out="$tmp/resp"
  if [ -n "$f" ]; then
    code="$(printf 'header = "Authorization: Bearer %s"\n' "$TOKEN" |
      curl -sS -K - -o "$out" -w '%{http_code}' --max-time 30 -X "$m" -H 'content-type: application/json' \
        --data-binary "@$f" "$API$p" 2>/dev/null)" || code=000
  else
    code="$(printf 'header = "Authorization: Bearer %s"\n' "$TOKEN" |
      curl -sS -K - -o "$out" -w '%{http_code}' --max-time 30 -X "$m" "$API$p" 2>/dev/null)" || code=000
  fi
  cat "$out" 2>/dev/null
  [ "$code" -lt 400 ] 2>/dev/null && [ "$(jq -r '.success' "$out" 2>/dev/null)" = true ]
}

# --- zone and plan ---
cf GET "/zones?name=$ZONE_NAME" >"$tmp/zone.json" || die "cannot read zone $ZONE_NAME (token scope or login?)"
ZONE_ID="$(jq -r '.result[0].id // empty' "$tmp/zone.json")"
[ -n "$ZONE_ID" ] || die "zone $ZONE_NAME not visible to this token"
PLAN="$(jq -r '.result[0].plan.legacy_id // .result[0].plan.name // "unknown"' "$tmp/zone.json" | tr '[:upper:]' '[:lower:]')"
case "$PLAN" in
  pro|business|enterprise) PERIOD=60; REQS=20; MITIGATION=600 ;;
  *) PERIOD=10; REQS=5; MITIGATION=10 ;;
esac
LIMIT_DESC="$REQS requests / ${PERIOD}s, block ${MITIGATION}s (plan: $PLAN)"
say "zone $ZONE_NAME found; rate limit for this plan: $LIMIT_DESC"

# --- scope check (read-only probes; warns, never prints the token) ---
broad=0
if [ "$from_wrangler" -eq 1 ]; then
  warn "using wrangler's OAuth login, which is broader than Zone WAF + Zone Settings on $ZONE_NAME"; broad=1
fi
nz="$(cf GET '/zones?per_page=50' | jq -r '[.result[]?.name] | map(select(. != "'"$ZONE_NAME"'")) | length' 2>/dev/null)"
[ "${nz:-0}" -gt 0 ] 2>/dev/null && { warn "token can see $nz zone(s) besides $ZONE_NAME"; broad=1; }
if cf GET '/accounts?per_page=5' >"$tmp/acc.json" && [ "$(jq -r '.result | length' "$tmp/acc.json")" -gt 0 ]; then
  warn "token has account-level access"; broad=1
fi
cf GET "/zones/$ZONE_ID/dns_records?per_page=1" >/dev/null && { warn "token can read DNS (not needed)"; broad=1; }
cf GET "/zones/$ZONE_ID/workers/routes" >/dev/null && { warn "token can read Workers routes (not needed)"; broad=1; }
[ "$broad" -eq 1 ] && warn "prefer a token with only Zone WAF:Edit + Zone Settings:Edit on $ZONE_NAME (--token-prompt)"

# --- wanted state ---
jq -n --arg ref "$RULE_REF" --arg expr "$EXPR" --argjson p "$PERIOD" --argjson r "$REQS" --argjson m "$MITIGATION" '{
  ref: $ref, description: "QRT-316 dash enroll", expression: $expr, action: "block", enabled: true,
  action_parameters: { response: { status_code: 429, content_type: "application/json",
    content: ({error: "rate limited", retry_after_ms: ($m * 1000)} | tojson) } },
  ratelimit: { characteristics: ["ip.src", "cf.colo.id"], period: $p, requests_per_period: $r,
    mitigation_timeout: $m } }' >"$tmp/rule.json"

read_rules() { # existing rules of the http_ratelimit entrypoint, [] if none
  if cf GET "/zones/$ZONE_ID/rulesets/phases/http_ratelimit/entrypoint" >"$tmp/rs.json"; then
    jq '[.result.rules[]?]' "$tmp/rs.json"
  else echo '[]'; fi
}
# Rule fields Cloudflare echoes back, in the shape we compare.
norm_rule() { jq -S '{ref, expression, action, enabled, response: .action_parameters.response,
  characteristics: (.ratelimit.characteristics | sort), period: .ratelimit.period,
  requests_per_period: .ratelimit.requests_per_period, mitigation_timeout: .ratelimit.mitigation_timeout}'; }
get_setting() { cf GET "/zones/$ZONE_ID/settings/$1" | jq -c '.result.value'; }
WANT_HSTS='{"enabled":true,"include_subdomains":false,"max_age":'"$HSTS_MAX_AGE"',"nosniff":true,"preload":false}'
hsts_now() { get_setting security_header | jq -c -S '.strict_transport_security | {enabled, include_subdomains, max_age, nosniff, preload}'; }

show_state() {
  local rules mine
  rules="$(read_rules)"
  mine="$(echo "$rules" | jq -c --arg ref "$RULE_REF" '.[] | select(.ref == $ref)')"
  if [ -n "$mine" ]; then say "rate-limit rule now: $(echo "$mine" | norm_rule | jq -c '{period, requests_per_period, mitigation_timeout}')"
  else say "rate-limit rule now: none"; fi
  say "always_use_https now: $(get_setting always_use_https)"
  say "min_tls_version now:  $(get_setting min_tls_version)"
  say "hsts now:             $(hsts_now)"
}

put_rules() { # json array file
  jq '{rules: .}' "$1" >"$tmp/put.json"
  cf PUT "/zones/$ZONE_ID/rulesets/phases/http_ratelimit/entrypoint" "$tmp/put.json" >"$tmp/put.out" ||
    die "rate-limit rule write failed: $(jq -c '.errors' "$tmp/put.out" 2>/dev/null)"
}
# Other rules in the phase are kept as they are (only id/version/last_updated stripped).
others() { read_rules | jq --arg ref "$RULE_REF" '[.[] | select(.ref != $ref) | del(.id, .version, .last_updated)]'; }

patch_setting() { # name json-value
  jq -n --argjson v "$2" '{value: $v}' >"$tmp/set.json"
  cf PATCH "/zones/$ZONE_ID/settings/$1" "$tmp/set.json" >/dev/null || die "setting $1 write failed"
}

case "$mode" in
  dry)
    show_state
    say "wanted: rule $LIMIT_DESC; always_use_https \"on\"; min_tls_version \"1.2\"; hsts $WANT_HSTS"
    say "dry run: nothing written. Re-run with --apply."
    exit 0 ;;
  revert)
    others >"$tmp/keep.json"; put_rules "$tmp/keep.json"
    [ -z "$(read_rules | jq -c --arg ref "$RULE_REF" '.[] | select(.ref == $ref)')" ] || die "rule still present after revert"
    say "reverted: rule $RULE_REF removed (HTTPS, TLS 1.2 and HSTS left on)"
    exit 0 ;;
esac

# --- apply ---
others >"$tmp/keep.json"
jq --slurpfile r "$tmp/rule.json" '. + $r' "$tmp/keep.json" >"$tmp/all.json"
put_rules "$tmp/all.json"
patch_setting always_use_https '"on"'
patch_setting min_tls_version '"1.2"'
patch_setting security_header '{"strict_transport_security":'"$WANT_HSTS"'}'
say "applied rate limit: $LIMIT_DESC"

# --- read-back: every value must match exactly ---
bad=0
got="$(read_rules | jq -c --arg ref "$RULE_REF" '.[] | select(.ref == $ref)' | norm_rule)"
want="$(norm_rule <"$tmp/rule.json")"
if [ -n "$got" ] && [ "$(echo "$got" | jq -c -S .)" = "$(echo "$want" | jq -c -S .)" ]; then
  say "ok   rate-limit rule read back: $REQS / ${PERIOD}s, block ${MITIGATION}s"
else say "FAIL rate-limit rule read back: $(echo "$got" | jq -c . 2>/dev/null)"; bad=1; fi
chk() { # label got want
  if [ "$2" = "$3" ]; then say "ok   $1 $2"; else say "FAIL $1 $2 (want $3)"; bad=1; fi
}
chk always_use_https "$(get_setting always_use_https)" '"on"'
chk min_tls_version "$(get_setting min_tls_version)" '"1.2"'
chk hsts "$(hsts_now)" "$(echo "$WANT_HSTS" | jq -c -S .)"
[ "$bad" -eq 0 ] || die "read-back mismatch; check the dashboard (Security > WAF, SSL/TLS > Edge Certificates)"
say "all edge settings applied and verified. Next: bash scripts/smoke-prod.sh --edge"
