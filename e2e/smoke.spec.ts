import { _electron as electron, expect, test } from '@playwright/test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

// Runs the production Vite build (apps/desktop/.vite, produced by `npm run package`) on the
// stock Electron binary. The packaged app can't be driven by Playwright because the hardened
// fuses (EnableNodeCliInspectArguments=false) block its debugger hook, which is intended.
const appDir = path.join(__dirname, '..', 'apps', 'desktop');
if (!fs.existsSync(path.join(appDir, '.vite', 'build', 'main.cjs')))
  throw new Error('Run `npm run package` first');

test('boots, renders sidebar, and talks to main + worker over typed IPC', async () => {
  const userData = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-e2e-'));
  const app = await electron.launch({
    args: [appDir, ...(process.platform === 'linux' ? ['--no-sandbox'] : [])],
    env: {
      ...process.env,
      QRTZ_USER_DATA: userData,
      QRTZ_MOCK_HELPER: '1',
      QRTZ_DEVTOOLS: '0',
    },
  });
  const win = await app.firstWindow();
  const errors: string[] = [];
  win.on('pageerror', (e) => errors.push(e.message));

  await expect(win.getByTestId('nav-note')).toBeVisible();
  await expect(win.getByTestId('note-app')).toBeVisible();

  await win.screenshot({
    path: path.join(__dirname, '..', 'test-results', 'note.png'),
  });
  await win.getByTestId('nav-settings').click();
  await expect(win.getByText(/database: ok · schema v5 · fts5 on/)).toBeVisible(
    { timeout: 15_000 },
  );
  await expect(win.getByText(/native helper: running/)).toBeVisible();

  // Capability scoping: "gallery" (no capabilities) may not read config.
  const denied = await win.evaluate(async () => {
    const t = (
      window as unknown as {
        qrtzTransport: {
          invoke: (
            e: unknown,
          ) => Promise<{ ok: boolean; error?: { code: string } }>;
        };
      }
    ).qrtzTransport;
    return t.invoke({
      v: 1,
      id: 'x',
      channel: 'config:get',
      version: 1,
      subApp: 'gallery',
      payload: undefined,
    });
  });
  expect(denied).toMatchObject({ ok: false, error: { code: 'E_FORBIDDEN' } });

  // Renderer has no Node.
  expect(
    await win.evaluate(
      () => typeof (globalThis as { require?: unknown }).require,
    ),
  ).toBe('undefined');

  expect(errors).toEqual([]);
  await app.close();
});
