#!/usr/bin/env bash
# QRT-322: jared-setup-once.sh must verify the `release` environment (and that no signing secret lives anywhere else)
# before it uploads anything. gh, uname, security, wrangler and bun are stubs on PATH; nothing touches GitHub.
# Each case: the stub serves one bad field; the script must exit non-zero and the stub must record NO `secret set`.
set -euo pipefail
SRC="$(cd "$(dirname "$0")" && pwd)/jared-setup-once.sh"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fail=0; pass=0

# throwaway repo on main == origin/main, containing only the script
git init -q --bare "$T/origin.git"
git init -q -b main "$T/repo"; mkdir -p "$T/repo/scripts/setup"; cp "$SRC" "$T/repo/scripts/setup/"
git -C "$T/repo" -c user.name=t -c user.email=t@t add -A
git -C "$T/repo" -c user.name=t -c user.email=t@t commit -qm t
git -C "$T/repo" remote add origin "$T/origin.git"; git -C "$T/repo" push -q origin main

# inputs (test-generated; no real key)
mkdir -p "$T/home/.qrtz-keys" "$T/bin"
openssl req -x509 -newkey rsa:2048 -nodes -subj /CN=t -days 1 -keyout "$T/k.pem" -out "$T/c.pem" 2>/dev/null
openssl pkcs12 -export -inkey "$T/k.pem" -in "$T/c.pem" -passout pass:testpw -out "$T/t.p12"
printf -- '-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n' > "$T/t.p8"
printf 'dGVzdGtleQ==\n' > "$T/home/.qrtz-keys/app-update.key"

cat > "$T/bin/uname" <<'S'
#!/bin/sh
echo Darwin
S
cat > "$T/bin/security" <<'S'
#!/bin/sh
case "$*" in *-w*) printf testpw ;; esac
S
for w in wrangler bun; do printf '#!/bin/sh\necho "%s $*" >> "$STUB_LOG"\n' "$w" > "$T/bin/$w"; done
cat > "$T/bin/gh" <<'S'
#!/usr/bin/env bash
# gh stub. CASE picks one bad field; everything else is the good state.
set -euo pipefail
echo "gh $*" >> "$STUB_LOG"
[[ "$1" == auth ]] && exit 0
if [[ "$1" == secret ]]; then
  [[ "$2" == set ]] && { cat >/dev/null; exit 0; }
  echo '[]' | jq -r '.[].name'; exit 0
fi
shift  # api
method=GET; jqf=; path=
while (($#)); do case "$1" in
  -X) method="$2"; shift 2 ;; --jq) jqf="$2"; shift 2 ;; --input) cat >/dev/null; shift 2 ;; -f) shift 2 ;;
  *) path="$1"; shift ;; esac; done
path="${path%%\?*}"
[[ "$CASE" == gh_error && "$path" == repos/qrtz-ai/qrtz/environments/release && $method == GET ]] && { echo "HTTP 502" >&2; exit 1; }
[[ $method != GET ]] && exit 0
rev='{"type":"User","reviewer":{"id":42,"login":"jlgrimes"}}'
case "$path" in
  user) out='{"login":"jlgrimes","id":42}' ;;
  repos/qrtz-ai/qrtz/environments/release)
    [[ "$CASE" == extra_reviewer ]] && rev="$rev"',{"type":"User","reviewer":{"id":7,"login":"bot"}}'
    bypass=false; [[ "$CASE" == admin_bypass ]] && bypass=true
    pb=false; cb=true; [[ "$CASE" == branch_policy ]] && { pb=true; cb=false; }
    out='{"name":"release","can_admins_bypass":'$bypass',"protection_rules":[{"type":"required_reviewers","reviewers":['"$rev"']}],"deployment_branch_policy":{"protected_branches":'$pb',"custom_branch_policies":'$cb'}}' ;;
  */deployment-branch-policies)
    case "$CASE" in
      extra_policy) out='{"branch_policies":[{"id":1,"name":"app-v*","type":"tag"},{"id":2,"name":"main","type":"branch"}]}' ;;
      missing_policy) out='{"branch_policies":[]}' ;;
      *) out='{"branch_policies":[{"id":1,"name":"app-v*","type":"tag"}]}' ;; esac ;;
  repos/qrtz-ai/qrtz/actions/secrets)
    out='{"total_count":0,"secrets":[]}'; [[ "$CASE" == repo_secrets ]] && out='{"total_count":1,"secrets":[{"name":"FOO"}]}' ;;
  repos/qrtz-ai/qrtz/actions/organization-secrets)
    [[ "$CASE" == org_secrets_error ]] && { echo "HTTP 403" >&2; exit 1; }
    out='{"total_count":0,"secrets":[]}'; [[ "$CASE" == org_secret ]] && out='{"total_count":1,"secrets":[{"name":"AC_API_KEY"}]}' ;;
  repos/qrtz-ai/qrtz/environments) out='{"environments":[{"name":"release"},{"name":"preview"}]}' ;;
  repos/qrtz-ai/qrtz/environments/preview/secrets)
    out='{"secrets":[{"name":"OTHER"}]}'; [[ "$CASE" == other_env_secret ]] && out='{"secrets":[{"name":"MACOS_CERT_P12"}]}' ;;
  *) echo "stub: unexpected path $path" >&2; exit 1 ;;
esac
if [[ -n "$jqf" ]]; then jq -r "$jqf" <<<"$out"; else echo "$out"; fi
S
chmod +x "$T/bin/"*

run() { # case, args...
  local c="$1"; shift
  : > "$T/log"
  ( cd "$T/repo" && env PATH="$T/bin:$PATH" HOME="$T/home" STUB_LOG="$T/log" CASE="$c" \
      QRTZ_P12="$T/t.p12" QRTZ_ASC_P8="$T/t.p8" bash scripts/setup/jared-setup-once.sh "$@" ) >"$T/out" 2>&1
}
check() { if eval "$2"; then pass=$((pass+1)); echo "ok    $1"; else fail=$((fail+1)); echo "FAIL  $1"; cat "$T/out"; fi; }

run good --release-only && rc=0 || rc=$?
check "good readback -> secrets uploaded" '[[ $rc == 0 ]] && [[ $(grep -c "^gh secret set" "$T/log") == 6 ]]'
check "release-only never invokes wrangler/bun" '! grep -qE "^(wrangler|bun) " "$T/log"'
check "no secret value in output" '! grep -qE "testpw|dGVzdGtleQ|AAAA" "$T/out"'
check "env PUT happens before any secret set" '[[ $(grep -n "api -X PUT" "$T/log" | head -1 | cut -d: -f1) -lt $(grep -n "secret set" "$T/log" | head -1 | cut -d: -f1) ]]'
for c in extra_reviewer admin_bypass branch_policy extra_policy missing_policy gh_error repo_secrets org_secret other_env_secret; do
  run "$c" --release-only && rc=0 || rc=$?
  check "$c -> abort, no secret set" '[[ $rc != 0 ]] && ! grep -q "secret set" "$T/log"'
done
run org_secrets_error --release-only && rc=0 || rc=$?
check "org_secrets_error -> abort, no secret set, says ping Beve" '[[ $rc != 0 ]] && ! grep -q "secret set" "$T/log" && grep -qF "token can'"'"'t verify org secrets; don'"'"'t bypass, ping Beve" "$T/out" && grep -qF "repos/qrtz-ai/qrtz/actions/organization-secrets" "$T/out"'
# QRT-254: --dash-only skips sections 1-4 and reaches section 5 (repo wrangler stub fails whoami, so nothing deploys)
mkdir -p "$T/repo/services/dash-worker/node_modules/.bin"
printf '#!/bin/sh\necho "wrangler $*" >> "$STUB_LOG"\nexit 1\n' > "$T/repo/services/dash-worker/node_modules/.bin/wrangler"
chmod +x "$T/repo/services/dash-worker/node_modules/.bin/wrangler"
run good --dash-only && rc=0 || rc=$?
check "dash-only -> no gh secret/environment calls, no p12/p8 prompt, reaches dash" '! grep -qE "^gh (secret|api .*environments|api .*secrets)" "$T/log" && ! grep -qE "path to|found Developer|found App Store|p12 valid|p8 valid" "$T/out" && grep -q "^bun install" "$T/log" && grep -q "^wrangler whoami" "$T/log" && grep -qF "wrangler is not logged in" "$T/out"'
run good --dash-only --release-only && rc=0 || rc=$?
check "dash-only + release-only -> dies, nothing called" '[[ $rc != 0 ]] && [[ ! -s "$T/log" ]] && grep -qF "cannot be combined" "$T/out"'
run good --force --dash-only && rc=0 || rc=$?
check "dash-only + force -> dies, nothing called" '[[ $rc != 0 ]] && [[ ! -s "$T/log" ]] && grep -qF "cannot be combined" "$T/out"'
# QRT-254 first run: section 5 end to end with a repo wrangler stub. DCASE picks the Cloudflare state. Needs a tty
# (DASH_SETUP_TOKEN is shown on /dev/tty), so these cases run under util-linux `script`. Nothing touches Cloudflare.
printf 'node_modules/\n' > "$T/repo/.gitignore"
printf 'name = "qrtz-dash"\n' > "$T/repo/services/dash-worker/wrangler.toml"
mkdir -p "$T/repo/services/dash-worker/scripts"
printf '#!/bin/sh\necho "smoke-prod" >> "$STUB_LOG"\n' > "$T/repo/services/dash-worker/scripts/smoke-prod.sh"
git -C "$T/repo" -c user.name=t -c user.email=t@t add -A
git -C "$T/repo" -c user.name=t -c user.email=t@t commit -qm dash
git -C "$T/repo" push -q origin main
cat > "$T/repo/services/dash-worker/node_modules/.bin/wrangler" <<'S'
#!/usr/bin/env bash
set -euo pipefail
echo "wrangler $*" >> "$STUB_LOG"
three='[{"name":"DASH_ENROLL_KEY","type":"secret_text"},{"name":"DASH_PEPPER","type":"secret_text"},{"name":"DASH_SETUP_TOKEN","type":"secret_text"}]'
case "$1 ${2:-}" in
  "whoami ") exit 0 ;;
  "secret list")
    if [[ -e "$STUB_DIR/deployed" ]]; then echo "$three"; exit 0; fi
    case "$DCASE" in
      first)     printf '\n\xe2\x9c\x98 [ERROR] Worker "qrtz-dash" not found.\n\nIf this is a new Worker, run `wrangler deploy` first to create it.\n' >&2; exit 1 ;;
      other_worker) printf '\xe2\x9c\x98 [ERROR] Worker "qrtz-dash-old" not found.\n' >&2; exit 1 ;;
      auth_err)  printf '\xe2\x9c\x98 [ERROR] Authentication error [code: 10000]\n' >&2; exit 1 ;;
      net_err)   printf '\xe2\x9c\x98 [ERROR] fetch failed\n' >&2; exit 1 ;;
      existing)  echo "$three"; exit 0 ;;
    esac ;;
  "secret put") cat > /dev/null; echo "put" >> "$STUB_DIR/puts"; exit 0 ;;
  "deploy "*|"deploy --secrets-file")
    if [[ "$*" == *"--secrets-file /dev/stdin"* ]]; then
      cat /dev/stdin | jq -r 'keys | sort | join(",")' > "$STUB_DIR/deploykeys"
    fi
    touch "$STUB_DIR/deployed"; exit 0 ;;
esac
echo "stub: unexpected wrangler $*" >&2; exit 1
S
chmod +x "$T/repo/services/dash-worker/node_modules/.bin/wrangler"
drun() { # dcase -> runs --dash-only under a pty
  rm -rf "$T/sd"; mkdir -p "$T/sd"; : > "$T/log"
  script -qec "cd '$T/repo' && env PATH='$T/bin:$PATH' HOME='$T/home' STUB_LOG='$T/log' STUB_DIR='$T/sd' CASE=good DCASE='$1' bash scripts/setup/jared-setup-once.sh --dash-only" /dev/null </dev/null >"$T/out" 2>&1
}
drun first && rc=0 || rc=$?
check "first run (worker not found) -> one deploy carrying exactly the three secrets, no secret put, then verify + smoke" '[[ $rc == 0 ]] && [[ ! -e "$T/sd/puts" ]] && ! grep -q "^wrangler secret put" "$T/log" && [[ $(grep -c "^wrangler deploy" "$T/log") == 1 ]] && grep -qxF "wrangler deploy --secrets-file /dev/stdin" "$T/log" && [[ $(cat "$T/sd/deploykeys") == DASH_ENROLL_KEY,DASH_PEPPER,DASH_SETUP_TOKEN ]] && grep -q "dash secrets exactly three" "$T/out" && grep -qx "smoke-prod" "$T/log"'
check "first run -> secret list, then deploy, then secret list re-check before smoke" '[[ "$(grep -E "^(wrangler (secret list|deploy)|smoke-prod)" "$T/log" | sed "s/ --.*//" | tr "\n" ";")" == "wrangler secret list;wrangler deploy;wrangler secret list;smoke-prod;" ]]'
check "first run -> no secret value in wrangler argv" '! grep -qE "[0-9a-f]{32}" "$T/log"'
drun existing && rc=0 || rc=$?
check "existing worker -> unchanged path: no puts (all present), plain deploy, smoke" '[[ $rc == 0 ]] && [[ ! -e "$T/sd/puts" ]] && grep -qxF "wrangler deploy" "$T/log" && ! grep -q "secrets-file" "$T/log" && grep -q "skip  dash secret DASH_PEPPER" "$T/out" && grep -qx "smoke-prod" "$T/log"'
for c in auth_err net_err other_worker; do
  drun "$c" && rc=0 || rc=$?
  check "$c on secret list -> abort, no put, no deploy" '[[ $rc != 0 ]] && [[ ! -e "$T/sd/puts" ]] && ! grep -qE "^wrangler (secret put|deploy)" "$T/log" && grep -qF "wrangler secret list failed; nothing rotated" "$T/out"'
done
echo "jared-setup-once.test: $pass passed, $fail failed"
[[ $fail == 0 ]]
