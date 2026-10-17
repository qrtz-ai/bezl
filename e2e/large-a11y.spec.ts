import { axe } from './axe';
import {
  _electron as electron,
  expect,
  test,
  type Page,
} from '@playwright/test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const appDir = path.join(__dirname, '..', 'apps', 'desktop');
const shots = path.join(__dirname, '..', 'test-results');

async function launch(extra: Record<string, string>) {
  const env: Record<string, string> = {
    ...(process.env as Record<string, string>),
    QRTZ_USER_DATA: fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-big-')),
    QRTZ_MOCK_HELPER: '1',
    QRTZ_DEVTOOLS: '0',
    ...extra,
  };
  const app = await electron.launch({
    args: [appDir, ...(process.platform === 'linux' ? ['--no-sandbox'] : [])],
    env,
  });
  return { app, win: await app.firstWindow() };
}

test('2-hour meeting: list, detail, transcript and search stay fast', async () => {
  const { app, win } = await launch({ QRTZ_DEV_SEED: 'large' });
  await win.getByTestId('nav-memo').click();
  const item = win.getByTestId('meeting-item').filter({ hasText: 'All-hands' });
  await expect(item).toBeVisible({ timeout: 15_000 });
  let t = Date.now();
  await item.click();
  await win.getByTestId('tab-summary').click(); // transcript is the default tab
  await expect(win.getByTestId('detail-summary')).toBeVisible();
  const openMs = Date.now() - t;
  t = Date.now();
  await win.getByTestId('tab-transcript').click();
  await expect(win.getByTestId('transcript-segment')).toHaveCount(1500);
  const transcriptMs = Date.now() - t;
  // Seek near the end and confirm the active row updates without re-rendering the world.
  await win.getByTestId('transcript-segment').nth(1400).click();
  await expect(
    win.locator('[data-testid="transcript-segment"][data-active]'),
  ).toHaveCount(1);
  t = Date.now();
  const search = win.getByTestId('memo-search');
  await search.click();
  await search.pressSequentially('migration');
  await expect(search).toHaveValue('migration');
  // Poll via evaluate: xvfb windows count as occluded, so Chromium throttles the renderer's
  // debounce timer unless the page is poked (not an issue in a focused window).
  await expect(async () => {
    await win.evaluate(() => new Promise((r) => setTimeout(r, 50)));
    expect(await win.getByTestId('search-snippet').count()).toBeGreaterThan(0);
  }).toPass({ timeout: 10_000 });
  const searchMs = Date.now() - t;
  await win.screenshot({ path: path.join(shots, 'large-meeting.png') });
  console.log(JSON.stringify({ openMs, transcriptMs, searchMs }));
  expect(openMs).toBeLessThan(3000);
  expect(transcriptMs).toBeLessThan(3000);
  expect(searchMs).toBeLessThan(2000);
  await app.close();
});

test('accessibility: no serious axe violations on main views', async () => {
  const { app, win } = await launch({});
  const bad: string[] = [];
  await expect(win.getByTestId('home')).toBeVisible();
  bad.push(...(await axe(win, 'home')));
  for (const id of ['memo', 'gallery', 'settings']) {
    await win.getByTestId(`nav-${id}`).click();
    await win.waitForTimeout(300);
    bad.push(...(await axe(win, id)));
    if (id === 'gallery') {
      await win
        .getByTestId('gallery-theme')
        .getByRole('tab', { name: 'Light' })
        .click();
      await win.waitForTimeout(300);
      bad.push(...(await axe(win, 'gallery')).map((v) => `light: ${v}`));
    }
  }
  await win.keyboard.press('ControlOrMeta+k');
  await expect(win.getByTestId('palette')).toBeVisible();
  bad.push(...(await axe(win, 'palette')));
  expect(bad).toEqual([]);
  await app.close();
});

test('cold start: launch to first paint of home under 1.5 s', async () => {
  const t0 = Date.now();
  const { app, win } = await launch({});
  await expect(win.getByTestId('home')).toBeVisible();
  const coldMs = Date.now() - t0;
  const paint = await win.evaluate(
    () =>
      performance.getEntriesByName('first-contentful-paint')[0]?.startTime ??
      -1,
  );
  console.log(JSON.stringify({ coldMs, firstContentfulPaintMs: paint }));
  expect(coldMs).toBeLessThan(
    Number(process.env.QRTZ_COLD_START_BUDGET_MS ?? 1500),
  );
  await app.close();
});
