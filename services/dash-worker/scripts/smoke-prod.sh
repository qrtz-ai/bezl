#!/bin/bash
# Post-deploy smoke test for https://dash.qrtz.ai (DEPLOY.md step 6). curl only, no secrets, bash 3.2 compatible.
# Read-only except one enroll attempt with a deliberately wrong code (counts as one failure for your IP).
# Usage: bash scripts/smoke-prod.sh [origin]   (default https://dash.qrtz.ai)
set -u
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
if [ "$s" = 200 ] && grep -q 'readme_version' "$body_file"; then ok "GET /v1/readme 200"; else bad "GET /v1/readme: $s (want 200)"; fi

# 2. A wrong code gets the uniform 401. Body built with printf (no escaped quotes inside a command substitution).
enroll_body="$(printf '{"code":"%s","name":"%s"}' 'ENR-ZZZZ-ZZZZ' 'smoke-test')"
s="$(status POST "$origin/v1/enroll" -H 'content-type: application/json' --data "$enroll_body")"
if [ "$s" = 401 ] && grep -q 'used, expired or wrong' "$body_file"; then
  ok "POST /v1/enroll wrong code 401 (uniform text)"
elif [ "$s" = 429 ]; then
  bad "POST /v1/enroll: 429 (this IP is rate limited or locked; retry later from another network)"
else
  bad "POST /v1/enroll wrong code: $s (want 401 'used, expired or wrong')"
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

if [ "$fails" -eq 0 ]; then echo "smoke: all checks passed for $origin"; exit 0; fi
echo "smoke: $fails check(s) failed for $origin"; exit 1
