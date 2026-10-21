/**
 * Local test mode (docs/dash-local.md, `bun run dev:local` in services/dash-worker). FOR BOZNIAK REVIEW (option b). On only when ALL hold:
 * - DASH_DEV_LOCAL is "1". It exists only in the gitignored .dev.vars that scripts/dev.sh (dev:local) writes; wrangler.toml
 *   never sets it (a test checks), and nothing in a request (header, Host, query) can turn it on.
 * - The connection came from loopback: cf-connecting-ip is 127.0.0.1 or ::1. In production Cloudflare sets that
 *   header to the real client address; locally `wrangler dev --ip 127.0.0.1` only accepts loopback connections.
 * - The URL host is exactly 127.0.0.1 (the only origin the dev app accepts).
 * In test mode plain http is accepted and enrollment codes are issued with TEST_CODE_PREFIX. Outside it, any code
 * with that prefix is refused like a wrong code (and counted as one).
 */
import type { Env } from './env';

export const TEST_CODE_PREFIX = 'TST-';

export const isLoopbackPeer = (ip: string | null) =>
  ip === '127.0.0.1' || ip === '::1';

export const devLocal = (env: Env, req: Request) =>
  env.DASH_DEV_LOCAL === '1' &&
  isLoopbackPeer(req.headers.get('cf-connecting-ip')) &&
  new URL(req.url).hostname === '127.0.0.1';

/** Inside a Durable Object (no request): the var alone, used only to choose the code prefix. */
export const devCodes = (env: Env) => env.DASH_DEV_LOCAL === '1';

/** The readme's rule 1 as published (docs/dash/agent-readme.md). A test checks it is in README_MD verbatim. */
export const README_RULE_1 =
  '1. HTTPS only, one origin: `https://dash.qrtz.ai`. Never send tokens anywhere else.';

const DEV_ORIGIN = /^http:\/\/127\.0\.0\.1:([1-9][0-9]{0,4})$/;

/**
 * QRT-298: the origin this local server serves from, or null. Only in local test mode, and only from the
 * DASH_DEV_ORIGIN var that scripts/lib.sh writes into the gitignored .dev.vars: never from the Host header or any
 * other part of the request. Exactly http://127.0.0.1:<1-65535>.
 */
export const devOrigin = (env: Env, req: Request): string | null => {
  if (!devLocal(env, req)) return null;
  const o = env.DASH_DEV_ORIGIN ?? '';
  const m = DEV_ORIGIN.exec(o);
  return m && Number(m[1]) <= 65535 ? o : null;
};

/** The readme to serve: production's as is; in local test mode, rule 1 names the local origin instead. */
export const readmeFor = (md: string, origin: string | null) =>
  origin === null
    ? md
    : md.replace(
        README_RULE_1,
        `1. One origin: \`${origin}\` (a local test server). Never send tokens anywhere else.`,
      );
