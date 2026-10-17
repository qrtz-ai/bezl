import { defineConfig } from '@playwright/test';

// The on-device embedding model downloads ~23 MB on first use; keep it off by default so specs stay hermetic.
// chat.spec.ts turns it on with a shared model cache.
process.env.QRTZ_EMBEDDINGS ??= '0';
// Shrink every lifecycle retry backoff (docs/recording-lifecycle.md) so failure paths settle quickly.
process.env.QRTZ_RETRY_SCALE ??= '0.05';

export default defineConfig({
  testDir: '.',
  timeout: 60_000,
  retries: 0,
  reporter: 'list',
});
