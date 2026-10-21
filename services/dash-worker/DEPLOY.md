# Dash worker: production deploy runbook (dash.qrtz.ai)

**Run this on Jared's Mac only.** Never from the box, CI, or any agent, and never through the Cloudflare
connector. Secret values are generated and piped locally and are never printed, pasted into chat, or committed.
Every command below runs from `services/dash-worker` with this package's own scripts; no root scripts.

## 0. Prerequisites

- [ ] `main` contains QRT-284 (per-code enrollment failure limit) and the Bozniak-cleared Dash changes. Do not deploy
      a tree without them: `git log --oneline origin/main | grep QRT-284` must print a line.
- [ ] Clean checkout of `main` at the cleared head: `git status` clean, `git rev-parse HEAD` noted for rollback.
- [ ] Node 22+ (`node -v`) and bun. Install deps: `cd services/dash-worker && bun install`.
- [ ] Log in to the Cloudflare account that owns the `qrtz.ai` zone: `npx wrangler login`, then
      `npx wrangler whoami` and check the account name. If several accounts are listed, export
      `CLOUDFLARE_ACCOUNT_ID=<that account id>` in this shell (an id, not a secret).
- [ ] Tests and dry-run build pass: `bun run test && bun run typecheck && bun run check:build`.
- [ ] Only route-level request logs will ship (QRT-307). Workers Logs keeps exactly one line per request, written by
      the Worker (`src/reqlog.ts`): `{"method","route","status","ms"}`, where `route` is the route template
      (e.g. `/v1/owner/agents/:id`, `/v1/widgets/:key`; unknown paths are `unmatched`). Never
      the real path, query string, full URL, headers (so no IP, user agent or bearer), or request/response bodies.
      `grep -n -A12 '\[observability\]' wrangler.toml` shows `enabled = true`, `head_sampling_rate = 1`,
      `redact_query_string = true`, `[observability.logs]` with `invocation_logs = false` (Cloudflare's automatic
      per-request log, which carries the URL and headers, stays off) and `[observability.traces] enabled = false`;
      there is no `[env.` section. After deploy, in the dashboard check that Workers Logs for `qrtz-dash` shows only
      those JSON lines and no invocation (URL) entries; if any appear, set `enabled = false` again and redeploy.
- [ ] No local-mode config will ship: `wrangler.toml` must not mention `DASH_DEV_LOCAL` or have a `[vars]` table
      (`grep -n 'DASH_DEV_LOCAL\|\[vars\]' wrangler.toml` prints nothing; `test/dev.test.ts` also checks this).
      `.dev.vars` is gitignored and only read by `wrangler dev`; it is never uploaded by `wrangler deploy`.

## What the worker needs (from `wrangler.toml` and `src/env.ts`)

| Name               | Kind                       | Notes                                                                                                                                                                                                 |
| ------------------ | -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `DASHBOARD`        | Durable Object binding     | class `Dashboard` (SQLite), migration `v1`                                                                                                                                                            |
| `ENROLL`           | Durable Object binding     | class `EnrollDirectory` (SQLite), migration `v1`                                                                                                                                                      |
| `DASH_PEPPER`      | secret, required           | exactly 64 hex chars (32 bytes). HMAC key for tokens, owner keys, codes, IPs. Missing/bad → every request 503 `server not configured`.                                                                |
| `DASH_SETUP_TOKEN` | secret, required           | ≥ 32 chars. One-time bearer for `POST /v1/owner/setup`; one dashboard per token, ever.                                                                                                                |
| `DASH_ENROLL_KEY`  | secret, required (QRT-284) | exactly 64 hex chars (32 bytes). HMAC key for enrollment codes only: stored form is HMAC(DASH_ENROLL_KEY, "enroll-code:" id "." secret). Missing/bad → `POST /v1/enroll` 503 `server not configured`. |
| `DASH_DEV_LOCAL`   | local only, NEVER in prod  | only in `.dev.vars` from `bun run dev:local`. Must not be set as a var or secret.                                                                                                                     |

Exactly three secrets: `DASH_PEPPER`, `DASH_SETUP_TOKEN` and `DASH_ENROLL_KEY` (added by QRT-284, so deploy only once QRT-284 is on `main`). Do not create any others.

## 1. Secrets

Generate and pipe in one step so the value never hits the terminal or shell history:

```bash
cd services/dash-worker
openssl rand -hex 32 | tr -d '\n' | npx wrangler secret put DASH_PEPPER
openssl rand -hex 32 | tr -d '\n' | npx wrangler secret put DASH_ENROLL_KEY
openssl rand -hex 24 | tr -d '\n' | tee >(pbcopy) | npx wrangler secret put DASH_SETUP_TOKEN
npx wrangler secret list          # names only; expect exactly DASH_PEPPER, DASH_ENROLL_KEY, DASH_SETUP_TOKEN
```

- `DASH_PEPPER` is permanent: rotating it invalidates every agent token and owner key. Set it once.
- `DASH_ENROLL_KEY` keys only enrollment codes. Rotating it kills outstanding (unused, 15-minute) codes and nothing
  else; enrolled agents keep working.
- `DASH_SETUP_TOKEN` goes onto your clipboard (via `pbcopy`) and is never echoed. It is one-time: right after the
  deploy and smoke test (sections 4 and 5), connect the app to `https://dash.qrtz.ai` and paste it to create the
  dashboard. Then clear the clipboard (`pbcopy </dev/null`) and rotate it to an unknown value:
  `openssl rand -hex 24 | tr -d '\n' | npx wrangler secret put DASH_SETUP_TOKEN`.
- Never `echo` a secret, never pass one as a command-line argument, never put one in `wrangler.toml`.
- If `secret put` runs before the first deploy, wrangler may offer to create the worker; that is fine (it is empty).

## 2. DNS / route for dash.qrtz.ai

`wrangler.toml` already declares `routes = [{ pattern = "dash.qrtz.ai", custom_domain = true }]`, with
`workers_dev = false` and `preview_urls = false`. On `wrangler deploy`, Cloudflare creates the proxied DNS record and
edge certificate for `dash.qrtz.ai` in the `qrtz.ai` zone. Do not add a manual DNS record or a separate route.

- [ ] Before deploy: Dashboard → qrtz.ai → DNS: there must be **no existing** `dash` record (a conflicting record makes
      custom-domain attach fail). Remove it only if you know it's stale.
- [ ] After deploy: Workers & Pages → `qrtz-dash` → Settings → Domains & Routes shows `dash.qrtz.ai` (Custom domain).
      DNS shows `dash` as a Worker record, proxied (orange cloud). No `*.workers.dev` URL is enabled.
- [ ] HTTPS only, minimum TLS 1.2 and HSTS are set by `scripts/edge.sh` (section 3). If they are off, the worker
      itself answers http with 400 `https only`, never content.

## 3. Edge settings: rate limit on /v1/enroll, HTTPS, TLS 1.2, HSTS (QRT-316)

The worker already limits wrong codes per IP, per /48 and per code (`src/constants.ts`, `src/directory.ts`; QRT-284:
5 wrong secrets kill a code, 10 wrong codes/min per dashboard and per IP get 429). The edge rule is the outer layer so
floods never reach the Durable Object. Everything here is applied by `scripts/edge.sh` from this Mac, never by hand
from another machine, the box or the Cloudflare connector.

```bash
cd services/dash-worker
bash scripts/edge.sh                     # dry run: current vs wanted, writes nothing
bash scripts/edge.sh --token-prompt      # same, with a scoped token (Zone WAF:Edit + Zone Settings:Edit on qrtz.ai)
bash scripts/edge.sh --apply             # (add --token-prompt) applies, prints the limit used, reads everything back
```

- Rate-limit rule `qrt316-enroll` on `POST dash.qrtz.ai/v1/enroll`, per `ip.src` + `cf.colo.id`: **20 requests /
  60 s, block 600 s** if the zone's plan allows a 60 s period, otherwise **5 / 10 s, block 10 s**. Blocked requests
  get `429 {"error":"rate limited","retry_after_ms":...}`. The script prints which limit it applied and fails unless
  that exact rule reads back. Other rate-limit rules in the zone are kept.
- Always Use HTTPS on, minimum TLS 1.2, HSTS max-age 15552000 (180 days) without includeSubDomains or preload.
- Without `--token-prompt` it uses your wrangler login and warns that it is broader than needed; it also warns if a
  pasted token can see other zones, accounts, DNS or Workers routes. The token is never printed or stored.
- Undo the rule only: `bash scripts/edge.sh --revert` (HTTPS/TLS/HSTS stay; HSTS can't be recalled from browsers).
- Then run `bash scripts/smoke-prod.sh --edge` (section 5).

## 4. Deploy

```bash
cd services/dash-worker
bun run check:build               # dry run, no upload
npx wrangler deploy               # uploads qrtz-dash, applies DO migration v1, attaches dash.qrtz.ai
npx wrangler deployments list     # note the new version id (and the previous one, for rollback)
```

There is no package `deploy` script on purpose; this is the only place `wrangler deploy` is run.

## 5. Smoke test

```bash
cd services/dash-worker
bash scripts/smoke-prod.sh        # defaults to https://dash.qrtz.ai
```

Checks (curl only, no secrets): `GET /v1/readme` is 200; `POST /v1/enroll` with a wrong code is 401 with
`used, expired or wrong`; plain `http://dash.qrtz.ai` is redirected or refused, never served; `POST /v1/owner/test-codes`
is 404. Exit 0 means all passed. It makes one wrong-code attempt from your IP (counted by the worker; harmless).

After `scripts/edge.sh --apply`, run `bash scripts/smoke-prod.sh --edge`: it also checks the http→https redirect, that
TLS 1.1 is refused and 1.2 works, the HSTS header, and that the `/v1/enroll` rule trips with a JSON 429. It trips the
rule with non-JSON POSTs (the worker answers 422 before any code is checked, so they are not wrong codes); the run
still sends only the one wrong code. It blocks `/v1/enroll` from your IP for up to 10 minutes: run it when no agent is
enrolling.

Then immediately connect the app and create the dashboard with the `DASH_SETUP_TOKEN` on your clipboard, clear the clipboard, and rotate `DASH_SETUP_TOKEN` (section 1). A 503 from the smoke test means a secret is missing: run `npx wrangler secret list`.

## 6. Rollback

```bash
cd services/dash-worker
npx wrangler deployments list               # find the last good version id
npx wrangler rollback <version-id>          # or `npx wrangler rollback` for the previous version
bash scripts/smoke-prod.sh
```

- Rollback restores code only. Secrets, Durable Object data and migrations are not rolled back; never roll back past
  the deployment that applied migration `v1`.
- To take Dash offline entirely: Workers → `qrtz-dash` → Domains & Routes → remove `dash.qrtz.ai` (data kept).
- If a secret leaked: rotate `DASH_SETUP_TOKEN` or `DASH_ENROLL_KEY` freely; rotating `DASH_PEPPER` logs out every agent and owner (needs
  Bozniak and Jared).
