import { defineConfig, devices } from '@playwright/test';

// Tauri renderer e2e: the same React UI the Tauri app ships, served by Vite with mocked IPC
// (apps/desktop-tauri/src/mock/backend.ts behind the real createTauriTransport). Chromium and WebKit
// (Playwright WebKit approximates macOS WKWebView). The real Rust backend is covered by `cargo test`.
const port = Number(process.env.QRTZ_TAURI_E2E_PORT ?? 1420);
export default defineConfig({
  testDir: '.',
  timeout: 30_000,
  retries: 0,
  reporter: 'list',
  use: {
    baseURL: `http://localhost:${port}`,
    viewport: { width: 1100, height: 720 },
  },
  webServer: {
    command: `npx vite --port ${port} --strictPort`,
    cwd: '../apps/desktop-tauri',
    env: { VITE_QRTZ_MOCK_IPC: '1' },
    url: `http://localhost:${port}`,
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
  projects: [
    {
      name: 'chromium',
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 1100, height: 720 },
      },
    },
    {
      name: 'webkit',
      use: {
        ...devices['Desktop Safari'],
        viewport: { width: 1100, height: 720 },
      },
    },
  ],
});
