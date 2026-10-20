import { cloudflareTest } from '@cloudflare/vitest-plugin';
import { readFileSync } from 'node:fs';
import { defineConfig } from 'vitest/config';

const repo = (p: string) =>
  readFileSync(new URL(`../../${p}`, import.meta.url), 'utf8');

// Test-only secrets (never real ones): a fixed pepper so the shared HMAC vectors are reproducible.
export default defineConfig({
  plugins: [
    cloudflareTest({
      wrangler: { configPath: './wrangler.toml' },
      miniflare: {
        bindings: {
          DASH_PEPPER:
            '00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff',
          // QRT-284: test placeholder for the enrollment-code HMAC key (a Worker secret in production).
          DASH_ENROLL_KEY:
            'e0e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9fafbfcfdfeff',
          DASH_SETUP_TOKEN: 'test-setup-token-not-a-real-secret-0000',
          // Read on the Node side and handed in: the shared vectors and the Rust constants (twin check).
          TEST_VECTORS: repo('contracts/dash/vectors.json'),
          TEST_RUST_LIB: repo('crates/qrtz-dash/src/lib.rs'),
          // The production config, to check it can't turn on local test mode (test/dev.test.ts).
          TEST_WRANGLER_TOML: repo('services/dash-worker/wrangler.toml'),
        },
      },
    }),
  ],
  test: { include: ['test/**/*.test.ts'] },
});
