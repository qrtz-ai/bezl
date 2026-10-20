import type { Dashboard } from './dashboard';
import type { EnrollDirectory } from './directory';

export interface Env {
  DASHBOARD: DurableObjectNamespace<Dashboard>;
  ENROLL: DurableObjectNamespace<EnrollDirectory>;
  /** Worker secret: 64 hex characters (32 bytes). Keys every token, owner key, code and IP HMAC. */
  DASH_PEPPER: string;
  /**
   * Worker secret (QRT-284): 64 hex characters (32 bytes), separate from DASH_PEPPER. Keys only the enrollment-code
   * secret HMAC (src/crypto.ts codeMac). Production: `wrangler secret put DASH_ENROLL_KEY`; locally a throwaway
   * value in the gitignored .dev.vars (scripts/lib.sh) or the fixed test placeholder in vitest.config.ts.
   */
  DASH_ENROLL_KEY: string;
  /** Worker secret: the one-time bearer for POST /v1/owner/setup (creating a dashboard). */
  DASH_SETUP_TOKEN: string;
  /** Local test mode only (src/dev.ts): "1" from `wrangler dev --var`. Never set in production. */
  DASH_DEV_LOCAL?: string;
}
