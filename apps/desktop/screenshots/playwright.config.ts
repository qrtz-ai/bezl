import { defineConfig, devices } from '@playwright/test';

// Screenshot comparisons for the page stories (QRT-304). One browser (chromium), one Linux image in CI, so
// font rendering is stable. More than 2% differing pixels fails; baselines change only with
// `bun run screenshots:update`. Each run's own screenshots are kept under test-results/screenshots.
// `bun run e2e` runs it first, so the existing CI playwright job covers it with no workflow change.
const port = 6126;
export default defineConfig({
  testDir: '.',
  // Under the repo's test-results/ so the CI playwright job uploads the diffs on failure.
  outputDir: '../../../test-results/screenshots',
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
        // Times on the pages (echo history, nano dates) render the same on the box and on CI.
        timezoneId: 'UTC',
        locale: 'en-US',
      },
    },
  ],
});
