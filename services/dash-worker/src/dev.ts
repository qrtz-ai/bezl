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
