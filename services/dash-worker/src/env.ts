import type { Dashboard } from './dashboard';
import type { EnrollDirectory } from './directory';

export interface Env {
  DASHBOARD: DurableObjectNamespace<Dashboard>;
  ENROLL: DurableObjectNamespace<EnrollDirectory>;
  /** Worker secret: 64 hex characters (32 bytes). Keys every token, owner key, code and IP HMAC. */
  DASH_PEPPER: string;
  /** Worker secret: the one-time bearer for POST /v1/owner/setup (creating a dashboard). */
  DASH_SETUP_TOKEN: string;
  /** Local test mode only (src/dev.ts): "1" from `wrangler dev --var`. Never set in production. */
  DASH_DEV_LOCAL?: string;
}
