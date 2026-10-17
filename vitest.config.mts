import { defineConfig } from 'vitest/config';
import { runTmpDir } from './scripts/test-tmp.mjs';

// Every test's `os.tmpdir()` is this run's private dir, removed when the run ends (N50, ADR 0040).
const runTmp = runTmpDir();

export default defineConfig({
  test: {
    include: [
      'packages/**/src/**/*.test.{ts,tsx}',
      'apps/*/src/**/*.test.{ts,tsx}',
      'scripts/**/*.test.mjs',
    ],
    environment: 'node',
    env: { TMPDIR: runTmp, TMP: runTmp, TEMP: runTmp, QRTZ_TEST_TMP: runTmp },
    globalSetup: ['./scripts/test-tmp.mjs'],
  },
});
