#!/bin/bash
# QRT-316: offline test of scripts/edge.sh against a mocked Cloudflare API (fake curl, uname and npx on PATH).
# No network, no real token. bash 3.2 compatible. Usage: bash scripts/edge.test.sh
set -u
here="$(cd "$(dirname "$0")" && pwd -P)"
EDGE="$here/edge.sh"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
FAKE_TOKEN="fake-token-NOT-REAL-7f3a91c2"
fails=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }

mkdir -p "$T/bin"
cat >"$T/bin/uname" <<'M'
#!/bin/bash
echo "${MOCK_OS:-Darwin}"
M
cat >"$T/bin/npx" <<'M'
#!/bin/bash
[ "${MOCK_NO_WRANGLER:-0}" = 1 ] && exit 1
echo "$MOCK_TOKEN"
M
# Fake curl: supports -K - (header from stdin), -o, -w '%{http_code}', -X, -H, --data-binary @file, --max-time, URL.
cat >"$T/bin/curl" <<'M'
#!/bin/bash
S="$MOCK_STATE"; out=/dev/stdout; m=GET; data=""; url=""
while [ $# -gt 0 ]; do
  case "$1" in
    -K) cat >"$S/last_config"; shift ;;
    -o) out="$2"; shift ;;
    -X) m="$2"; shift ;;
    -w|-H|--max-time) shift ;;
    --data-binary) data="${2#@}"; shift ;;
    -*) ;;
    *) url="$1" ;;
  esac
  shift
done
grep -q "Bearer $MOCK_TOKEN\"" "$S/last_config" || { echo '{"success":false}' >"$out"; printf 403; exit 0; }
p="${url#*client/v4}"
echo "$m $p" >>"$S/log"
[ -n "$data" ] && cp "$data" "$S/body.$(wc -l <"$S/log" | tr -d ' ')"
ok() { jq -c -n --argjson r "$1" '{success:true, result:$r}' >"$out"; printf 200; }
no() { echo '{"success":false,"errors":[{"code":9109}]}' >"$out"; printf "${1:-403}"; }
case "$m $p" in
  "GET /zones?name=qrtz.ai") ok "[{\"id\":\"z1\",\"name\":\"qrtz.ai\",\"plan\":{\"legacy_id\":\"$MOCK_PLAN\"}}]" ;;
  "GET /zones?per_page=50")
    if [ "${MOCK_BROAD:-0}" = 1 ]; then ok '[{"name":"qrtz.ai"},{"name":"other.com"}]'; else ok '[{"name":"qrtz.ai"}]'; fi ;;
  "GET /accounts?per_page=5") if [ "${MOCK_BROAD:-0}" = 1 ]; then ok '[{"id":"a"}]'; else no; fi ;;
  "GET /zones/z1/dns_records?per_page=1"|"GET /zones/z1/workers/routes") if [ "${MOCK_BROAD:-0}" = 1 ]; then ok '[]'; else no; fi ;;
  "GET /zones/z1/rulesets/phases/http_ratelimit/entrypoint")
    if [ -f "$S/rules" ]; then ok "{\"rules\":$(cat "$S/rules")}"; else no 404; fi ;;
  "PUT /zones/z1/rulesets/phases/http_ratelimit/entrypoint")
    jq '[.rules[] | . + {id:"r", version:"1"}]' "$data" >"$S/rules"; ok "{\"rules\":$(cat "$S/rules")}" ;;
  "GET /zones/z1/settings/"*)
    n="${p##*/}"; if [ -f "$S/set.$n" ]; then ok "{\"value\":$(cat "$S/set.$n")}"; else ok '{"value":"off"}'; fi ;;
  "PATCH /zones/z1/settings/"*)
    n="${p##*/}"; v="$(jq -c .value "$data")"
    [ "${MOCK_BAD_READBACK:-0}" = 1 ] && [ "$n" = min_tls_version ] && v='"1.0"'
    echo "$v" >"$S/set.$n"; ok "{\"value\":$v}" ;;
  *) no 404 ;;
esac
M
chmod +x "$T/bin/"*

# run NAME [env...] -- [args]: runs edge.sh with a fresh state; stdout+stderr in $T/NAME.out, exit in $T/NAME.rc
run() {
  local name="$1"; shift
  local envs=()
  while [ "$1" != -- ]; do envs+=("$1"); shift; done; shift
  rm -rf "$T/state"; mkdir -p "$T/state"; : >"$T/state/log"
  [ -n "${SEED_RULES:-}" ] && echo "$SEED_RULES" >"$T/state/rules"
  env PATH="$T/bin:$PATH" MOCK_STATE="$T/state" MOCK_TOKEN="$FAKE_TOKEN" MOCK_PLAN=free \
    CF_API_BASE="https://api.example.test/client/v4" "${envs[@]+"${envs[@]}"}" \
    bash "$EDGE" "$@" <"${RUN_STDIN:-/dev/null}" >"$T/$name.out" 2>&1
  echo $? >"$T/$name.rc"
  cp "$T/state/log" "$T/$name.log"
}
rc() { cat "$T/$1.rc"; }
has() { grep -qF -- "$2" "$T/$1.out"; }
writes() { grep -cE '^(PUT|PATCH|POST|DELETE) ' "$T/$1.log" | tr -d ' '; }
no_token() { if grep -qF "$FAKE_TOKEN" "$T/$1.out"; then bad "$1: token appeared in output"; else ok "$1: token never printed"; fi; }

# 1. Refuses anywhere but macOS, before touching the API.
run linux MOCK_OS=Linux -- --apply
[ "$(rc linux)" != 0 ] && has linux "Jared's Mac only" && [ ! -s "$T/linux.log" ] && ok "refuses on Linux, no API calls" || bad "linux: $(cat "$T/linux.out")"

# 2. Dry run writes nothing.
run dry --
[ "$(rc dry)" = 0 ] && [ "$(writes dry)" = 0 ] && has dry "dry run: nothing written" && ok "dry run: no writes" || bad "dry: $(cat "$T/dry.out")"
no_token dry

# 3. Free plan: 5 / 10 s, block 10 s, printed and verified on read-back.
run free -- --apply
if [ "$(rc free)" = 0 ] && has free "applied rate limit: 5 requests / 10s, block 10s (plan: free)" \
  && has free "ok   rate-limit rule read back: 5 / 10s, block 10s"; then ok "free plan: 5/10s applied and read back"; else bad "free: $(cat "$T/free.out")"; fi
body="$(grep -l '"rules"' "$T"/state/body.* 2>/dev/null | head -n1)"
if [ -n "$body" ] && [ "$(jq -c '.rules[0].ratelimit' "$body")" = '{"characteristics":["ip.src","cf.colo.id"],"period":10,"requests_per_period":5,"mitigation_timeout":10}' ] \
  && [ "$(jq -r '.rules[0].expression' "$body")" = '(http.host eq "dash.qrtz.ai" and http.request.uri.path eq "/v1/enroll" and http.request.method eq "POST")' ] \
  && [ "$(jq -r '.rules[0].action_parameters.response.content' "$body")" = '{"error":"rate limited","retry_after_ms":10000}' ] \
  && [ "$(jq -r '.rules[0].action_parameters.response.status_code' "$body")" = 429 ]; then ok "free plan: exact rule payload"
else bad "free: rule payload $(cat "$body" 2>/dev/null)"; fi
for s in 'always_use_https "on"' 'min_tls_version "1.2"' 'hsts {"enabled":true,"include_subdomains":false,"max_age":15552000,"nosniff":true,"preload":false}'; do
  has free "ok   $s" && ok "free: $s verified" || bad "free: $s not verified"
done
has free "WARNING: using wrangler's OAuth login" && ok "warns that the wrangler login is broader" || bad "no wrangler-scope warning"
no_token free

# 4. Pro plan: 20 / 60 s, block 600 s.
run pro MOCK_PLAN=pro -- --apply
[ "$(rc pro)" = 0 ] && has pro "applied rate limit: 20 requests / 60s, block 600s (plan: pro)" \
  && has pro "ok   rate-limit rule read back: 20 / 60s, block 600s" && ok "pro plan: 20/60s applied and read back" || bad "pro: $(cat "$T/pro.out")"

# 5. Read-back mismatch fails.
run mismatch MOCK_BAD_READBACK=1 -- --apply
[ "$(rc mismatch)" != 0 ] && has mismatch 'FAIL min_tls_version "1.0"' && has mismatch "read-back mismatch" && ok "read-back mismatch exits non-zero" || bad "mismatch: $(cat "$T/mismatch.out")"

# 6. Broad token warnings; scoped token from the hidden prompt is never printed.
printf '%s\n' "$FAKE_TOKEN" >"$T/tok"; RUN_STDIN="$T/tok" run broad MOCK_BROAD=1 MOCK_NO_WRANGLER=1 -- --token-prompt
has broad "zone(s) besides qrtz.ai" && has broad "account-level access" && has broad "can read DNS" && ok "broad token: warned" || bad "broad: $(cat "$T/broad.out")"
rm -rf "$T/state"; mkdir -p "$T/state"; : >"$T/state/log"
printf '%s\n' "$FAKE_TOKEN" | env PATH="$T/bin:$PATH" MOCK_STATE="$T/state" MOCK_TOKEN="$FAKE_TOKEN" MOCK_PLAN=free \
  CF_API_BASE="https://api.example.test/client/v4" bash "$EDGE" --token-prompt >"$T/prompt.out" 2>&1
pr=$?
[ "$pr" = 0 ] && ! grep -q "WARNING" "$T/prompt.out" && ok "scoped token via prompt: no scope warnings" || bad "prompt: $(cat "$T/prompt.out")"
no_token prompt

# 7. Other rules in the phase survive apply and revert; revert removes only ours.
SEED_RULES='[{"id":"x","version":"3","ref":"someone-else","expression":"true","action":"block","ratelimit":{"characteristics":["ip.src"],"period":10,"requests_per_period":99,"mitigation_timeout":10}}]'
run keep -- --apply
[ "$(rc keep)" = 0 ] && [ "$(jq -r '[.[].ref] | sort | join(",")' "$T/state/rules")" = "qrt316-enroll,someone-else" ] && ok "apply keeps other rules" || bad "keep: $(cat "$T/state/rules" 2>/dev/null)"
SEED_RULES="$(cat "$T/state/rules")"
run revert -- --revert
[ "$(rc revert)" = 0 ] && [ "$(jq -r '[.[].ref] | join(",")' "$T/state/rules")" = "someone-else" ] && ok "revert removes only qrt316-enroll" || bad "revert: $(cat "$T/revert.out")"
unset SEED_RULES

# 8. Bad argument.
run badarg -- --yolo
[ "$(rc badarg)" = 2 ] && ok "unknown argument rejected" || bad "badarg rc $(rc badarg)"

if [ "$fails" -eq 0 ]; then echo "edge.test: all passed"; exit 0; fi
echo "edge.test: $fails failed"; exit 1
