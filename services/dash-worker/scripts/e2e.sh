#!/usr/bin/env bash
# bun run --cwd services/dash-worker e2e: a self-contained smoke test of the local dash server. Starts its own `wrangler dev --local` on
# 127.0.0.1:8798 with a THROWAWAY state dir and secrets file (deleted at exit; values never printed; separate from
# dev:local's), then runs setup -> code -> enroll -> approve -> heartbeat -> widget -> owner view.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
need_node22; worker_deps
PORT="${DASH_E2E_PORT:-8798}"; B="http://127.0.0.1:$PORT"
tmp="$(mktemp -d)"; chmod 700 "$tmp"
write_vars "$tmp/e2e.vars" "$PORT"
SETUP="$(sed -n 's/^DASH_SETUP_TOKEN=//p' "$tmp/e2e.vars")"
( run_wrangler "$PORT" "$tmp/state" --env-file "$tmp/e2e.vars" --inspector-port 0 ) >"$tmp/dev.log" 2>&1 &
PID=$!
stop() { set +e; pkill -f "persist-to $tmp/state" 2>/dev/null; pkill -TERM -P $PID 2>/dev/null; kill $PID 2>/dev/null; wait $PID 2>/dev/null; rm -rf "$tmp"; return 0; }
trap stop EXIT
for _ in $(seq 90); do kill -0 $PID 2>/dev/null || break; curl -fsS -m 2 "$B/v1/readme" >/dev/null 2>&1 && break; sleep 1; done
curl -fsS -m 2 "$B/v1/readme" >/dev/null 2>&1 || { echo "wrangler dev did not start:"; tail -20 "$tmp/dev.log"; exit 1; }
j() { node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>console.log(JSON.parse(s)[process.argv[1]]))' "$1"; }
req() { curl -sS -m 10 -o "$tmp/out" -w '%{http_code}' -X "$1" "$B$2" -H 'content-type: application/json' "${@:3}"; }
check() { if [ "$1" = "$2" ]; then echo "ok   $3 ($1)"; else echo "FAIL $3: got $1 want $2: $(cat "$tmp/out")"; exit 1; fi; }
check "$(req GET /v1/readme)" 200 "GET /v1/readme over http on 127.0.0.1"
# QRT-298: in local mode rule 1 names this server's origin (from e2e.vars), not https://dash.qrtz.ai.
RULE1="$(grep -m1 'Never send tokens anywhere else' "$tmp/out" || true)"
case "$RULE1" in
  *"\`$B\`"*) echo "ok   readme rule 1 names $B" ;;
  *) echo "FAIL readme rule 1: $RULE1"; exit 1 ;;
esac
if grep -q 'https://dash.qrtz.ai`. Never send' "$tmp/out"; then echo "FAIL readme still has the production rule 1"; exit 1; else echo "ok   no production rule 1 in local mode"; fi
check "$(req POST /v1/owner/setup -H 'authorization: Bearer nope')" 401 "setup with a wrong token"
check "$(req POST /v1/owner/setup -H "authorization: Bearer $SETUP")" 201 "setup"
OWNER="$(j owner_key <"$tmp/out")"
check "$(req POST /v1/owner/setup -H "authorization: Bearer $SETUP")" 410 "setup is one-time"
check "$(req POST /v1/owner/codes -H "authorization: Bearer $OWNER")" 201 "app endpoint issues a normal code"
[[ "$(j code <"$tmp/out")" == ENR-* ]] && echo "ok   /v1/owner/codes gives ENR-" || { echo "FAIL /v1/owner/codes prefix"; exit 1; }
check "$(req POST /v1/owner/test-codes -H "authorization: Bearer $OWNER")" 201 "issue test code (local mode only)"
CODE="$(j code <"$tmp/out")"
[[ "$CODE" == TST-* ]] && echo "ok   code is a test code (TST-)" || { echo "FAIL code prefix: ${CODE:0:4}"; exit 1; }
check "$(req POST /v1/enroll -d '{"code":"ENR-AAAA-BBBB","name":"x"}')" 401 "wrong code"
check "$(req POST /v1/enroll -d "{\"code\":\"$CODE\",\"name\":\"smoke bot\",\"description\":\"local smoke\"}")" 201 "enroll"
AID="$(j agent_id <"$tmp/out")"; TOK="$(j agent_token <"$tmp/out")"
check "$(req POST /v1/enroll -d "{\"code\":\"$CODE\",\"name\":\"again\"}")" 401 "code is single use"
check "$(req POST /v1/heartbeat -H "authorization: Bearer $TOK" -d '{"status":"idle"}')" 403 "pending agent is refused"
check "$(req POST "/v1/owner/agents/$AID/review" -H "authorization: Bearer $OWNER" -d '{"approve":true}')" 200 "owner approves agent"
check "$(req POST /v1/heartbeat -H "authorization: Bearer $TOK" -d '{"status":"idle"}')" 200 "approved heartbeat"
check "$(req POST /v1/widgets -H "authorization: Bearer $TOK" -d '{"key":"smoke","type":"status","title":"Smoke"}')" 201 "create widget"
check "$(req GET /v1/owner/view -H "authorization: Bearer $OWNER")" 200 "owner view"
grep -q '"smoke"' "$tmp/out" && echo "ok   widget visible in owner view" || { echo "FAIL widget missing from owner view"; exit 1; }
echo "e2e: all passed"
