import { defineConfig, devices } from '@playwright/test';

// Screenshot comparisons for the page stories (QRT-304). One browser (chromium), one Linux image in CI, so
// font rendering is stable. A difference fails; baselines change only with `bun run screenshots:update`.
const port = 6126;
export default defineConfig({
  testDir: '.',
  outputDir: './results',
  snapshotPathTemplate: '{testDir}/baselines/{arg}{ext}',
  timeout: 30_000,
  retries: 0,
  reporter: 'list',
  updateSnapshots: 'none',
  expect: { toHaveScreenshot: { animations: 'disabled', caret: 'hide' } },
  use: { baseURL: `http://localhost:${port}` },
  webServer: {
    command: `bun ./screenshots/serve.ts ${port}`,
    cwd: '..',
    url: `http://localhost:${port}/iframe.html`,
    reuseExistingServer: false,
    timeout: 30_000,
  },
  projects: [
    {
      name: 'chromium',
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 1100, height: 720 },
      },
    },
  ],
});
