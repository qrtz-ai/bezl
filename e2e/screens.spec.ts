import {
  _electron as electron,
  expect,
  test,
  type Page,
} from '@playwright/test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

// First-run walkthrough. Screenshots go to test-results/screens, or docs/screenshots with QRTZ_DOC_SHOTS=1
// (how the README images are refreshed).
const appDir = path.join(__dirname, '..', 'apps', 'desktop');
const out = process.env.QRTZ_DOC_SHOTS
  ? path.join(__dirname, '..', 'docs', 'screenshots')
  : path.join(__dirname, '..', 'test-results', 'screens');

async function shot(win: Page, name: string) {
  fs.mkdirSync(out, { recursive: true });
  await win.evaluate(() => document.fonts.ready.then(() => undefined));
  await win.screenshot({ path: path.join(out, `${name}.png`) });
}

test('first run → record → review walkthrough', async () => {
  const env: Record<string, string> = {
    ...(process.env as Record<string, string>),
    QRTZ_USER_DATA: fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-screens-')),
    QRTZ_MOCK_HELPER: '1',
    QRTZ_MOCK_SCENARIO: 'standup',
    QRTZ_MOCK_SPEED: '0.06',
    QRTZ_DEVTOOLS: '0',
  };
  delete env.QRTZ_SECRET_OPENROUTER;
  delete env.QRTZ_SECRET_LLM_OPENROUTER;
  const app = await electron.launch({
    args: [appDir, ...(process.platform === 'linux' ? ['--no-sandbox'] : [])],
    env,
  });
  const win = await app.firstWindow();
  await app.evaluate(({ BrowserWindow }) =>
    BrowserWindow.getAllWindows()[0]?.setContentSize(1280, 800),
  );
  const errors: string[] = [];
  win.on('pageerror', (e) => errors.push(e.message));

  await expect(win.getByTestId('home')).toBeVisible();
  await shot(win, 'home-first-run');

  await win.getByTestId('nav-memo').click();
  await expect(win.getByTestId('memo-detected')).toBeVisible({
    timeout: 10_000,
  });
  await shot(win, 'memo-detected');
  await win.getByTestId('memo-record').click();
  await expect(async () => {
    await win.evaluate(() => new Promise((r) => setTimeout(r, 50)));
    expect(await win.getByTestId('memo-live-line').count()).toBeGreaterThan(4);
  }).toPass({ timeout: 15_000 });
  await shot(win, 'memo-live');
  await win.getByTestId('memo-stop').click();
  await expect(win.getByTestId('memo-done')).toBeVisible({ timeout: 20_000 });
  await win.getByTestId('memo-open').click();
  await expect(win.getByTestId('detail-summary')).toBeVisible();
  await shot(win, 'meeting-summary');
  await win.getByTestId('tab-transcript').click();
  await shot(win, 'meeting-transcript');

  await win.getByTestId('nav-home').click();
  await expect(win.getByTestId('home')).toBeVisible();
  await shot(win, 'home-with-meetings');
  await win.getByTestId('nav-settings').click();
  await expect(win.getByRole('heading', { name: 'Settings' })).toBeVisible();
  await expect(win.getByText(/Native helper:/)).toBeVisible();
  await shot(win, 'settings');
  await win.keyboard.press('ControlOrMeta+k');
  await win.getByTestId('palette-input').fill('onboarding');
  await expect(async () => {
    await win.evaluate(() => new Promise((r) => setTimeout(r, 50)));
    await expect(win.getByText('Weekly Product Sync').last()).toBeVisible({
      timeout: 100,
    });
  }).toPass({ timeout: 10_000 });
  await shot(win, 'palette');
  expect(errors).toEqual([]);
  await app.close();
});
