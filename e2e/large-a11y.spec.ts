import { createRequire } from 'node:module';
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

/** Serious/critical axe violations (WCAG 2.1 A/AA) on the current view. */
// axe-core is evaluated over CDP (not a <script> tag), so the app's strict CSP stays untouched.
const axeSource = fs.readFileSync(
  createRequire(__filename).resolve('axe-core/axe.min.js'),
  'utf8',
);
async function axe(win: Page, name: string) {
  await win.evaluate(axeSource);
  const r = await win.evaluate(() =>
    (
      window as unknown as {
        axe: {
          run: (
            c: unknown,
            o: unknown,
          ) => Promise<{
            violations: {
              id: string;
              impact: string;
              help: string;
              nodes: { target: unknown[]; failureSummary?: string }[];
            }[];
          }>;
        };
      }
    ).axe.run(document, {
      runOnly: {
        type: 'tag',
        values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'],
      },
    }),
  );
  const bad = r.violations
    .filter((v) => v.impact === 'serious' || v.impact === 'critical')
    .map((v) => `${name}: ${v.id} (${v.nodes.length}) ${v.help}`);
  for (const v of r.violations.filter(
    (v) => v.impact === 'serious' || v.impact === 'critical',
  ))
    for (const n of v.nodes)
      console.log(
        name,
        v.id,
        JSON.stringify(n.target),
        n.failureSummary?.replace(/\s+/g, ' '),
      );
  return bad;
}

test('2-hour meeting: list, detail, transcript and search stay fast', async () => {
  const { app, win } = await launch({ QRTZ_DEV_SEED: 'large' });
  await win.getByTestId('nav-memo').click();
  const item = win.getByTestId('meeting-item').filter({ hasText: 'All-hands' });
  await expect(item).toBeVisible({ timeout: 15_000 });
  let t = Date.now();
  await item.click();
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
  }
  await win.keyboard.press('ControlOrMeta+k');
  await expect(win.getByTestId('palette')).toBeVisible();
  bad.push(...(await axe(win, 'palette')));
  expect(bad).toEqual([]);
  await app.close();
});
