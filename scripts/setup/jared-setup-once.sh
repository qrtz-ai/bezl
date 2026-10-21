#!/usr/bin/env bash
# One-time (idempotent) setup, run by Jared on his Mac from ~/code/qrtz. Never on the box or in CI.
#   scripts/setup/jared-setup-once.sh                 create/verify what's missing, skip what exists
#   scripts/setup/jared-setup-once.sh --force         also rotate the GitHub `release` environment secrets
#   scripts/setup/jared-setup-once.sh --rotate-dash   also rotate the Dash worker secrets (logs out every agent/owner)
# Prints item names and ok/skip only. No secret ever goes into argv, an env var, a temp file or output.
# Requires QRT-284 and QRT-307 on main. Reads ~/.qrtz-keys/app-update.key only; never ~/qrtz-root-a or qrtz-root-b.
set -euo pipefail
set +x
FORCE=0; ROTATE_DASH=0
for a in "$@"; do case "$a" in
  --force) FORCE=1 ;; --rotate-dash) ROTATE_DASH=1 ;; *) echo "unknown argument: $a" >&2; exit 2 ;;
esac; done
ok()   { printf 'ok    %s\n' "$1"; }
skip() { printf 'skip  %s\n' "$1"; }
die()  { printf 'FAIL  %s\n' "$1" >&2; exit 1; }

[[ "$(uname -s)" == Darwin ]] || die "run this on your Mac"
ROOT="$(git rev-parse --show-toplevel)"; cd "$ROOT"
[[ "$(git rev-parse --abbrev-ref HEAD)" == main ]] || die "check out main first"
# soft early warning only; the hard clean-tree + HEAD == origin/main guard runs right before wrangler deploy
if git fetch -q origin main && [[ "$(git rev-parse HEAD)" != "$(git rev-parse origin/main)" ]]; then
  printf 'warn  HEAD is not origin/main; the deploy step will refuse (git pull --ff-only)\n'
fi
REPO=qrtz-ai/qrtz
TAGS=(app-v\* nano-v\* ctrl-v\* snap-v\* v\*)
command -v gh >/dev/null || die "gh not installed"
gh auth status >/dev/null 2>&1 || die "gh is not logged in (gh auth login)"
ME_LOGIN="$(gh api user --jq .login)"; ME_ID="$(gh api user --jq .id)"

# ---- 1. environment `release` (confirmed before anything is uploaded) ----------------------------------------------
# prevent_self_review is false: Jared is the only reviewer and also starts releases. The real control is the bots'
# fine-grained token (no Actions/Deployments/Environments/Secrets access), so no bot can trigger or approve.
gh api -X PUT "repos/$REPO/environments/release" --input - >/dev/null <<JSON
{"wait_timer":0,"prevent_self_review":false,"can_admins_bypass":false,
 "reviewers":[{"type":"User","id":$ME_ID}],
 "deployment_branch_policy":{"protected_branches":false,"custom_branch_policies":true}}
JSON
ok "environment release"
pol_list() { gh api "repos/$REPO/environments/release/deployment-branch-policies" --jq '.branch_policies[] | "\(.type):\(.name)"'; }
have_pols="$(pol_list)"
for p in branch:main "${TAGS[@]/#/tag:}"; do
  if grep -qxF "$p" <<<"$have_pols"; then skip "deploy policy $p"; else
    gh api -X POST "repos/$REPO/environments/release/deployment-branch-policies" -f name="${p#*:}" -f type="${p%%:*}" >/dev/null
    ok "deploy policy $p"; fi
done
jq -e --argjson id "$ME_ID" '
  .can_admins_bypass == false
  and .deployment_branch_policy.custom_branch_policies == true
  and .deployment_branch_policy.protected_branches == false
  and ([.protection_rules[] | select(.type=="required_reviewers")] | length == 1)
  and ([.protection_rules[] | select(.type=="required_reviewers")][0].reviewers | map(.reviewer.id) == [$id])
' <<<"$(gh api "repos/$REPO/environments/release")" >/dev/null || die "release environment protections did not verify; nothing uploaded"
want="$(printf '%s\n' branch:main "${TAGS[@]/#/tag:}" | sort | paste -sd, -)"
got="$(pol_list | sort | paste -sd, -)"
[[ "$got" == "$want" ]] || die "release deploy policies are not exactly main, app-v*, nano-v*, ctrl-v*, snap-v*, v*; nothing uploaded"
ok "release protections verified (reviewer $ME_LOGIN, no admin bypass, main + release tags)"

# ---- 2. no repo-level secrets (checked before any upload) ---------------------------------------------------------
repo_secrets="$(gh secret list --repo "$REPO" --json name --jq '.[].name')" || die "gh secret list (repo) failed"
[[ -z "$repo_secrets" ]] || die "repo-level secrets exist; signing secrets must be environment-only (nothing uploaded)"
ok "no repo-level secrets"

# ---- 3. inputs: files (found or prompted, then validated) and keychain items (created with hidden prompts) ---------
pick() { # var, label, candidate (from env or the repo's documented path; may be empty)
  local p="$3"
  if [[ -z "$p" || ! -f "$p" ]]; then
    read -r -p "path to $2${3:+ [$3]}: " p </dev/tty || die "no path for $2"
    p="${p:-$3}"; p="${p/#\~/$HOME}"
  fi
  [[ -n "$p" && -f "$p" && -s "$p" ]] || die "$2 not found at '${p}'"
  printf -v "$1" '%s' "$p"; ok "found $2"
}
kc_item() { # service, label: create with security's own hidden prompt (value never in argv) if missing
  if security find-generic-password -s "$1" -a qrtz >/dev/null 2>&1; then skip "keychain $1"; return; fi
  printf 'enter %s (hidden, twice):\n' "$2" >/dev/tty
  security add-generic-password -s "$1" -a qrtz -U -w </dev/tty || die "could not create keychain item $1"
  ok "keychain $1"
}
pick P12 "Developer ID .p12 (only the MR26CCZ496 identity)" "${QRTZ_P12:-}"
pick P8  "App Store Connect API key .p8" "${QRTZ_ASC_P8:-}"
pick UPD "app-update key (scripts/ci/setup-secrets.sh)" "$HOME/.qrtz-keys/app-update.key"
kc_item qrtz-release-p12-password   "the .p12 export password"
kc_item qrtz-release-asc-key-id     "the App Store Connect key id"
kc_item qrtz-release-asc-issuer-id  "the App Store Connect issuer id"
kc() { security find-generic-password -s "$1" -a qrtz -w | tr -d '\n'; }
kc qrtz-release-p12-password | /usr/bin/openssl pkcs12 -info -noout -in "$P12" -passin stdin >/dev/null 2>&1 \
  || die "$P12 is not a .p12 that opens with the qrtz-release-p12-password password"
ok "p12 valid"
[[ "$(head -n1 "$P8")" == "-----BEGIN PRIVATE KEY-----" ]] || die "$P8 has no BEGIN PRIVATE KEY header"
ok "p8 valid"
[[ "$(tr -d '[:space:]' <"$UPD")" =~ ^[A-Za-z0-9+/]+=*$ ]] >/dev/null || die "$UPD is not a base64 minisign key"
ok "app-update key valid"

# ---- 4. signing secrets -> environment `release` (stdin only) -----------------------------------------------------
have="$(gh secret list --repo "$REPO" --env release --json name --jq '.[].name')" || die "gh secret list (env) failed"
need() { ! grep -qxF "$1" <<<"$have" || [[ $FORCE == 1 ]]; }
gset() { gh secret set "$1" --repo "$REPO" --env release >/dev/null; ok "secret $1"; }
if need MACOS_CERT_P12;      then base64 -i "$P12" | tr -d '\n' | gset MACOS_CERT_P12; else skip "secret MACOS_CERT_P12"; fi
if need MACOS_CERT_PASSWORD; then kc qrtz-release-p12-password | gset MACOS_CERT_PASSWORD; else skip "secret MACOS_CERT_PASSWORD"; fi
if need AC_API_KEY;          then gset AC_API_KEY <"$P8"; else skip "secret AC_API_KEY"; fi
if need AC_KEY_ID;           then kc qrtz-release-asc-key-id | gset AC_KEY_ID; else skip "secret AC_KEY_ID"; fi
if need AC_ISSUER_ID;        then kc qrtz-release-asc-issuer-id | gset AC_ISSUER_ID; else skip "secret AC_ISSUER_ID"; fi
if need APP_UPDATE_KEY;      then tr -d '[:space:]' <"$UPD" | gset APP_UPDATE_KEY; else skip "secret APP_UPDATE_KEY"; fi
gh secret list --repo "$REPO" --env release --json name --jq '.[].name' | sed 's/^/name  /'

# ---- 5. Dash (services/dash-worker/DEPLOY.md) ---------------------------------------------------------------------
cd "$ROOT/services/dash-worker"
bun install --frozen-lockfile >/dev/null; ok "bun install"
W=./node_modules/.bin/wrangler
[[ -x "$W" ]] || die "no repo wrangler at services/dash-worker/$W"
"$W" whoami >/dev/null 2>&1 || die "wrangler is not logged in: run '$W login' in services/dash-worker, then rerun"
ok "wrangler whoami"
if grep -nq 'DASH_DEV_LOCAL\|\[vars\]' wrangler.toml; then die "wrangler.toml has DASH_DEV_LOCAL or [vars] (DEPLOY.md §0)"; fi
ok "no local-mode config"
dash_json="$("$W" secret list --format json)" || die "wrangler secret list failed; nothing rotated"
have_d="$(jq -r '.[].name' <<<"$dash_json")" || die "could not parse wrangler secret list"
dneed() { ! grep -qxF "$1" <<<"$have_d" || [[ $ROTATE_DASH == 1 ]]; }
for s in DASH_PEPPER:32 DASH_ENROLL_KEY:32; do
  n="${s%%:*}"
  if dneed "$n"; then openssl rand -hex "${s#*:}" | tr -d '\n' | "$W" secret put "$n" >/dev/null; ok "dash secret $n"
  else skip "dash secret $n"; fi
done
if dneed DASH_SETUP_TOKEN; then
  tok="$(openssl rand -hex 32)"
  printf '\nDASH_SETUP_TOKEN (shown once, here only; write it down for owner setup):\n%s\n\n' "$tok" >/dev/tty
  printf '%s' "$tok" | "$W" secret put DASH_SETUP_TOKEN >/dev/null; tok=
  ok "dash secret DASH_SETUP_TOKEN"
else skip "dash secret DASH_SETUP_TOKEN"; fi
names="$("$W" secret list --format json | jq -r '[.[].name] | sort | join(",")')" || die "wrangler secret list failed"
[[ "$names" == DASH_ENROLL_KEY,DASH_PEPPER,DASH_SETUP_TOKEN ]] || die "dash secrets are not exactly the three (DEPLOY.md §1)"
ok "dash secrets exactly three"
bun run check:build >/dev/null; ok "check:build"
git -C "$ROOT" fetch -q origin main || die "git fetch origin main failed; nothing deployed"
[[ -z "$(git -C "$ROOT" status --porcelain --untracked-files=all)" ]] || die "working tree not clean (incl. untracked files); nothing deployed"
[[ "$(git -C "$ROOT" rev-parse HEAD)" == "$(git -C "$ROOT" rev-parse origin/main)" ]] || die "HEAD is not origin/main (git pull --ff-only); nothing deployed"
ok "clean tree at origin/main"
"$W" deploy >/dev/null; ok "deploy qrtz-dash + dash.qrtz.ai custom domain (DNS)"
bash scripts/smoke-prod.sh >/dev/null || die "smoke-prod failed (rollback: $W rollback)"
ok "smoke-prod dash.qrtz.ai"
