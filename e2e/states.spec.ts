import {
  _electron as electron,
  expect,
  test,
  type Page,
} from '@playwright/test';
import fs from 'node:fs';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import os from 'node:os';
import path from 'node:path';

// Empty/error states and list scale: helper missing, LLM failure + retry (local OpenAI-compatible stub),
// and paging through hundreds of meetings.
const appDir = path.join(__dirname, '..', 'apps', 'desktop');
const shots = path.join(__dirname, '..', 'test-results');

async function launch(extra: Record<string, string>) {
  const env: Record<string, string> = {
    ...(process.env as Record<string, string>),
    QRTZ_USER_DATA: fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-states-')),
    QRTZ_MOCK_HELPER: '1',
    QRTZ_DEVTOOLS: '0',
    ...extra,
  };
  if (!extra.QRTZ_SECRET_OPENROUTER) delete env.QRTZ_SECRET_OPENROUTER;
  const app = await electron.launch({
    args: [appDir, ...(process.platform === 'linux' ? ['--no-sandbox'] : [])],
    env,
  });
  const win = await app.firstWindow();
  const errors: string[] = [];
  win.on('pageerror', (e) => errors.push(e.message));
  return { app, win, errors };
}

/** xvfb windows are treated as occluded and their timers get throttled, so poke the page while polling. */
async function until(win: Page, fn: () => Promise<void>, timeout = 15_000) {
  await expect(async () => {
    await win.evaluate(() => new Promise((r) => setTimeout(r, 50)));
    await fn();
  }).toPass({ timeout });
}

test('helper missing: clear notice, recording disabled, app still usable', async () => {
  const { app, win, errors } = await launch({
    QRTZ_SIMULATE_MISSING_HELPER: '1',
  });
  await expect(win.getByTestId('helper-notice')).toContainText(
    'capture helper missing',
  );
  await win.screenshot({ path: path.join(shots, 'helper-missing.png') });
  await win.getByTestId('nav-note').click();
  await expect(win.getByTestId('helper-notice')).toBeVisible();
  await win.getByTestId('note-record-manual').click();
  await expect(win.getByTestId('note-error')).toContainText(
    'helper is missing',
  );
  await win.getByTestId('nav-settings').click();
  await expect(win.getByTestId('settings-error')).toHaveCount(0);
  expect(errors).toEqual([]);
  await app.close();
});

test('LLM failure falls back to on-device summary, Retry gets the AI summary', async () => {
  let fail = true;
  const server = http.createServer((req, res) => {
    req.resume();
    req.on('end', () => {
      if (fail) {
        res.writeHead(503, { 'content-type': 'application/json' });
        return res.end('{"error":{"message":"upstream overloaded"}}');
      }
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(
        JSON.stringify({
          choices: [
            {
              message: {
                role: 'assistant',
                content: JSON.stringify({
                  summary:
                    'STUB-AI: the team agreed to ship the beta on Friday.',
                  actionItems: [{ text: 'Ship the beta', owner: 'Ada' }],
                  qa: [],
                }),
              },
            },
          ],
        }),
      );
    });
  });
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  const port = (server.address() as AddressInfo).port;
  const { app, win, errors } = await launch({
    QRTZ_MOCK_SCENARIO: 'standup',
    QRTZ_MOCK_SPEED: '0.02',
    QRTZ_SECRET_OPENROUTER: 'sk-test-stub',
    QRTZ_LLM_BASE_URL: `http://127.0.0.1:${port}/api/v1`,
  });
  await win.getByTestId('nav-note').click();
  await expect(win.getByTestId('note-detected')).toBeVisible({
    timeout: 10_000,
  });
  await win.getByTestId('note-record').click();
  await until(win, async () => {
    expect(await win.getByTestId('live-segment').count()).toBeGreaterThan(3);
  });
  await win.getByTestId('note-stop').click();
  await until(win, async () => {
    expect(
      await win.getByTestId('meeting-item').first().innerText(),
    ).not.toMatch(/recording|processing/);
  });
  await win.getByTestId('meeting-item').first().click();
  await win.getByTestId('tab-summary').click(); // transcript is the default tab
  await expect(win.getByTestId('llm-error')).toContainText('503', {
    timeout: 15_000,
  });
  await win.screenshot({ path: path.join(shots, 'llm-error.png') });
  fail = false;
  await win.getByTestId('llm-retry').click();
  await until(win, async () => {
    await win.getByTestId('tab-summary').click(); // transcript is the default tab
    expect(await win.getByTestId('detail-summary').innerText()).toContain(
      'STUB-AI',
    );
  });
  await expect(win.getByTestId('llm-error')).toHaveCount(0);
  expect(errors).toEqual([]);
  await app.close();
  server.close();
});

test('hundreds of meetings: paged list stays fast', async () => {
  const { app, win, errors } = await launch({ QRTZ_DEV_SEED: 'many' });
  await win.getByTestId('nav-note').click();
  await until(win, async () => {
    expect(await win.getByTestId('meeting-item').count()).toBe(100);
  });
  const t = Date.now();
  await win.getByTestId('note-show-more').click();
  await until(win, async () => {
    expect(await win.getByTestId('meeting-item').count()).toBe(200);
  });
  const moreMs = Date.now() - t;
  for (let i = 0; i < 3; i++) await win.getByTestId('note-show-more').click();
  await until(win, async () => {
    expect(await win.getByTestId('meeting-item').count()).toBe(500);
  });
  await expect(win.getByTestId('note-show-more')).toHaveCount(0);
  await win.getByTestId('meeting-item').last().click();
  await expect(
    win.getByRole('heading', { name: 'Sync #1', exact: true }),
  ).toBeVisible();
  console.log(JSON.stringify({ moreMs }));
  expect(moreMs).toBeLessThan(2000);
  expect(errors).toEqual([]);
  await app.close();
});
