#!/bin/bash
# Post-deploy smoke test for https://dash.qrtz.ai (DEPLOY.md step 6). curl only, no secrets, bash 3.2 compatible.
# Read-only except one enroll attempt with a deliberately wrong code (counts as one failure for your IP).
# Usage: bash scripts/smoke-prod.sh [--edge] [origin]   (default https://dash.qrtz.ai)
# --edge (QRT-316, after scripts/edge.sh --apply): also checks the http redirect, TLS 1.2 minimum, HSTS and the
# /v1/enroll rate-limit rule. The rule is tripped with non-JSON POSTs, which the worker answers 422 before any code
# is checked, so they never count as wrong codes; the whole run sends exactly one wrong code (check 2, max 4).
set -u
edge=0
if [ "${1:-}" = "--edge" ]; then edge=1; shift; fi
origin="${1:-https://dash.qrtz.ai}"
host="${origin#https://}"
fails=0
body_file="$(mktemp)"
trap 'rm -f "$body_file"' EXIT

ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }

# status METHOD URL [extra curl args]: prints the HTTP status, body goes to $body_file.
status() {
  local m="$1" u="$2"; shift 2
  curl -sS -o "$body_file" -w '%{http_code}' --max-time 15 -X "$m" "$@" "$u" 2>/dev/null || echo 000
}

# 1. README is served over https.
s="$(status GET "$origin/v1/readme")"
if [ "$s" = 503 ]; then bad "GET /v1/readme: 503 server not configured (missing secret)"
elif [ "$s" = 200 ] && grep -q 'readme_version' "$body_file"; then ok "GET /v1/readme 200"; else bad "GET /v1/readme: $s (want 200)"; fi

# 2. A wrong code gets the uniform 401. Body built with printf (no escaped quotes inside a command substitution).
enroll_body="$(printf '{"code":"%s","name":"%s"}' 'ENR-ZZZZ-ZZZZ' 'smoke-test')"
s="$(status POST "$origin/v1/enroll" -H 'content-type: application/json' --data "$enroll_body")"
if [ "$s" = 401 ] && grep -q 'used, expired or wrong' "$body_file"; then
  ok "POST /v1/enroll wrong code 401 (uniform text)"
elif [ "$s" = 503 ]; then
  bad "POST /v1/enroll: 503 server not configured (missing secret: check npx wrangler secret list for DASH_PEPPER, DASH_ENROLL_KEY)"
elif [ "$s" = 429 ]; then
  bad "POST /v1/enroll: 429 (this IP is rate limited or locked; retry later from another network)"
else
  bad "POST /v1/enroll wrong code: $s (want exactly 401 'used, expired or wrong')"
fi

# 3. Plain http is never served: a redirect to https or the worker's 400 'https only' are both fine; 200 is not.
s="$(status GET "http://$host/v1/readme")"
case "$s" in
  301|302|307|308) ok "http redirected ($s)" ;;
  400) if grep -q 'https only' "$body_file"; then ok "http refused (400 https only)"; else bad "http: 400 without 'https only'"; fi ;;
  000) ok "http connection refused" ;;
  *) bad "http: $s (want redirect or refusal, never served)" ;;
esac

# 4. The local-test-mode endpoint does not exist in production.
s="$(status POST "$origin/v1/owner/test-codes")"
if [ "$s" = 404 ]; then ok "POST /v1/owner/test-codes 404"; else bad "POST /v1/owner/test-codes: $s (want 404)"; fi

if [ "$edge" -eq 1 ]; then
  # 5. Always Use HTTPS at the edge: a real redirect, not the worker's 400.
  s="$(status GET "http://$host/v1/readme")"
  case "$s" in 301|308) ok "edge: http -> https redirect ($s)" ;; *) bad "edge: http gave $s (want 301/308 from Always Use HTTPS)" ;; esac
  # 6. TLS 1.1 is refused (min TLS 1.2), TLS 1.2 works.
  if curl -sS -o /dev/null --max-time 15 --tlsv1.1 --tls-max 1.1 "$origin/v1/readme" 2>/dev/null; then bad "edge: TLS 1.1 accepted"
  else ok "edge: TLS 1.1 refused"; fi
  if curl -sS -o /dev/null --max-time 15 --tlsv1.2 --tls-max 1.2 "$origin/v1/readme" 2>/dev/null; then ok "edge: TLS 1.2 accepted"
  else bad "edge: TLS 1.2 failed"; fi
  # 7. HSTS header (edge-set; 180 days, no includeSubDomains/preload).
  hsts="$(curl -sS -D - -o /dev/null --max-time 15 "$origin/v1/readme" 2>/dev/null | tr -d '\r' | grep -i '^strict-transport-security:' || true)"
  case "$hsts" in
    *max-age=15552000*) case "$hsts" in *[Pp]reload*|*[Ii]nclude[Ss]ub[Dd]omains*) bad "edge: HSTS has preload/includeSubDomains: $hsts" ;; *) ok "edge: HSTS max-age=15552000" ;; esac ;;
    *) bad "edge: HSTS header missing or wrong: ${hsts:-none}" ;;
  esac
  # 8. Rate-limit rule: non-JSON POSTs (422 from the worker, never counted as wrong codes) until Cloudflare answers
  # 429 with the rule's JSON body. At most 25 requests (the paid-plan rule trips at 21, the free one at 6).
  echo "note: this blocks /v1/enroll from your IP for up to 10 minutes; run it when no agent is enrolling."
  tripped=0; n=0
  while [ "$n" -lt 25 ]; do
    n=$((n + 1))
    s="$(status POST "$origin/v1/enroll" -H 'content-type: application/json' --data 'smoke-not-json')"
    if [ "$s" = 429 ]; then tripped=1; break; fi
    if [ "$s" != 422 ]; then bad "edge: non-JSON POST $n gave $s (want 422 until the rule trips)"; break; fi
  done
  if [ "$tripped" = 1 ] && grep -q '"error":"rate limited"' "$body_file" && grep -q '"retry_after_ms":' "$body_file"; then
    ok "edge: /v1/enroll rate limit tripped after $n requests (429 JSON with retry_after_ms)"
  elif [ "$tripped" = 1 ]; then bad "edge: 429 after $n requests but not the rule's JSON body: $(head -c 200 "$body_file")"
  else bad "edge: rate limit never tripped in $n requests"; fi
fi

if [ "$fails" -eq 0 ]; then echo "smoke: all checks passed for $origin"; exit 0; fi
echo "smoke: $fails check(s) failed for $origin"; exit 1
