import { defineConfig } from '@playwright/test';

// The on-device embedding model downloads ~23 MB on first use; keep it off by default so specs stay hermetic.
// chat.spec.ts turns it on with a shared model cache.
process.env.QRTZ_EMBEDDINGS ??= '0';

export default defineConfig({
  testDir: '.',
  timeout: 60_000,
  retries: 0,
  reporter: 'list',
});
