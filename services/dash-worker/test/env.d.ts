import type { Env as DashEnv } from '../src/env';
declare global {
  namespace Cloudflare {
    interface GlobalProps {
      mainModule: typeof import('../src/index');
      durableNamespaces: 'Dashboard' | 'EnrollDirectory';
    }
    interface Env extends DashEnv {
      TEST_VECTORS: string;
      TEST_RUST_LIB: string;
      TEST_WRANGLER_TOML: string;
    }
  }
}
