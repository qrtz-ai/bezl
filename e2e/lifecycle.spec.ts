import {
  _electron as electron,
  expect,
  test,
  type ElectronApplication,
  type Page,
} from '@playwright/test';
import fs from 'node:fs';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import os from 'node:os';
import path from 'node:path';

// Recording lifecycle end to end (docs/recording-lifecycle.md) against the mock helper:
// live transcript streaming (partials → finals, auto-scroll pause), transcript after stop and after restart,
// permission denied, LLM failure → retries → fallback → manual retry, and app restart mid post-processing.
const appDir = path.join(__dirname, '..', 'apps', 'desktop');
const shots = path.join(__dirname, '..', 'test-results');

async function launch(userData: string, extra: Record<string, string> = {}) {
  const env: Record<string, string> = {
    ...(process.env as Record<string, string>),
    QRTZ_USER_DATA: userData,
    QRTZ_MOCK_HELPER: '1',
    QRTZ_MOCK_SCENARIO: 'standup',
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
async function until(win: Page, fn: () => Promise<void>, timeout = 20_000) {
  await expect(async () => {
    await win.evaluate(() => new Promise((r) => setTimeout(r, 30)));
    await fn();
  }).toPass({ timeout, intervals: [50, 100, 200] });
}

function llmStub() {
  const state = { fail: true, calls: 0 };
  const server = http.createServer((req, res) => {
    req.resume();
    req.on('end', () => {
      state.calls++;
      if (state.fail) {
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
                  actionItems: [],
                  qa: [],
                }),
              },
            },
          ],
        }),
      );
    });
  });
  return {
    state,
    server,
    async url() {
      await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
      return `http://127.0.0.1:${(server.address() as AddressInfo).port}/api/v1`;
    },
  };
}

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-lc-'));
async function record(win: Page) {
  await win.getByTestId('nav-memo').click();
  await expect(win.getByTestId('memo-detected')).toBeVisible({
    timeout: 10_000,
  });
  await win.getByTestId('memo-record').click();
}
const phases = (app: ElectronApplication) =>
  app.evaluate(
    () => (globalThis as { __qrtzPhases?: string[] }).__qrtzPhases ?? [],
  );

test('live transcript streams partials then finals, follows live, and is front and center after stop and restart', async () => {
  const userData = tmp();
  // ~3.6 s scripted meeting: slow enough to observe partials being replaced.
  const { app, win, errors } = await launch(userData, {
    QRTZ_MOCK_SPEED: '0.1',
  });
  await record(win);
  const view = win.getByTestId('meeting-detail');
  await expect(view).toHaveAttribute('data-live', '', { timeout: 10_000 });
  const transcript = win.getByTestId('live-transcript');
  await expect(transcript).toBeVisible();

  // A partial appears before its final, then is replaced in place by the finalized segment.
  const partial = win.getByTestId('live-partial');
  await until(win, async () => expect(await partial.count()).toBe(1), 10_000);
  const pid = await partial.getAttribute('data-id');
  const firstText = await partial.innerText();
  expect(pid).toBeTruthy();
  await until(win, async () => {
    expect(
      await win
        .locator(`[data-testid="live-segment"][data-id="${pid}"]`)
        .count(),
    ).toBe(1);
    expect(
      await win
        .locator(`[data-testid="live-partial"][data-id="${pid}"]`)
        .count(),
    ).toBe(0);
  });
  const finalText = await win
    .locator(`[data-testid="live-segment"][data-id="${pid}"]`)
    .innerText();
  expect(finalText.length).toBeGreaterThanOrEqual(firstText.length - 20);

  // Text arrives incrementally (several distinct observations of a growing transcript).
  const counts = new Set<number>();
  await until(win, async () => {
    counts.add(await win.getByTestId('live-segment').count());
    expect(counts.size).toBeGreaterThanOrEqual(3);
  });
  await win.screenshot({
    path: path.join(shots, 'lifecycle-live-transcript.png'),
  });

  // Scrolling up pauses auto-follow and offers "Jump to live"; jumping resumes following.
  await transcript.evaluate((el) => {
    el.style.maxHeight = '80px';
    el.scrollTop = 0;
    el.dispatchEvent(new Event('scroll'));
  });
  await until(win, async () => {
    await transcript.evaluate((el) => {
      el.scrollTop = 0;
      el.dispatchEvent(new Event('scroll'));
    });
    await expect(win.getByTestId('jump-to-live')).toBeVisible({ timeout: 500 });
  });
  await expect(transcript).not.toHaveAttribute('data-following', '');
  await win.getByTestId('jump-to-live').click();
  await expect(transcript).toHaveAttribute('data-following', '');

  // Let the whole script play, then stop: the persisted transcript is the default view.
  await until(win, async () =>
    expect(await win.getByTestId('live-segment').count()).toBe(8),
  );
  await win.getByTestId('memo-stop').click();
  await until(win, async () => {
    await expect(view).not.toHaveAttribute('data-live', '');
    expect(await win.getByTestId('transcript-segment').count()).toBe(8);
  });
  await expect(win.getByTestId('detail-transcript')).toBeVisible();
  expect(await phases(app)).toEqual(
    expect.arrayContaining([
      'detected',
      'starting',
      'recording',
      'stopping',
      'processing',
      'done',
    ]),
  );
  expect(errors).toEqual([]);
  await app.close();

  // After a restart the full transcript is still what the meeting opens on.
  const b = await launch(userData);
  await b.win.getByTestId('nav-memo').click();
  await b.win.getByTestId('meeting-item').first().click();
  await expect(b.win.getByTestId('detail-transcript')).toBeVisible();
  await until(b.win, async () =>
    expect(await b.win.getByTestId('transcript-segment').count()).toBe(8),
  );
  expect(b.errors).toEqual([]);
  await b.app.close();
});

test('permission denied: start refused as lifecycle state, no meeting created', async () => {
  const { app, win, errors } = await launch(tmp(), {
    QRTZ_MOCK_PERMISSIONS: 'screen-recording=denied,microphone=denied',
  });
  await win.getByTestId('nav-memo').click();
  await win.getByTestId('onboarding-continue').click();
  await expect(win.getByTestId('memo-detected')).toBeVisible({
    timeout: 10_000,
  });
  await win.getByTestId('memo-record').click();
  await expect(win.getByTestId('lifecycle-error')).toContainText(
    'Screen & System Audio Recording or Microphone',
  );
  await expect(win.getByTestId('memo-detected')).toBeVisible(); // phase unchanged
  await expect(win.getByTestId('meeting-item')).toHaveCount(0);
  await expect(win.getByTestId('memo-error')).toHaveCount(0); // not a generic error
  await win.screenshot({
    path: path.join(shots, 'lifecycle-permission-denied.png'),
  });
  await win.getByTestId('lifecycle-error-dismiss').click();
  await expect(win.getByTestId('lifecycle-error')).toHaveCount(0);
  expect(errors).toEqual([]);
  await app.close();
});

test('LLM failure: summary retries with backoff, falls back on-device, manual step retry succeeds', async () => {
  const llm = llmStub();
  const { app, win, errors } = await launch(tmp(), {
    QRTZ_MOCK_SPEED: '0.02',
    QRTZ_SECRET_OPENROUTER: 'sk-test-stub',
    QRTZ_LLM_BASE_URL: await llm.url(),
  });
  await record(win);
  await until(win, async () =>
    expect(await win.getByTestId('live-segment').count()).toBeGreaterThan(3),
  );
  await win.getByTestId('memo-stop').click();
  const step = win.getByTestId('pipeline-step-summary');
  await until(win, async () =>
    expect(await step.getAttribute('data-status')).toBe('failed'),
  );
  expect(llm.state.calls).toBe(3); // maxAttempts
  await expect(step).toContainText('on-device fallback');
  await expect(step).toContainText('503');
  await expect(win.getByTestId('pipeline-step-finalize')).toHaveAttribute(
    'data-status',
    'done',
  );
  await win.screenshot({ path: path.join(shots, 'lifecycle-llm-failed.png') });
  llm.state.fail = false;
  await win.getByTestId('pipeline-retry-summary').click();
  // Every step done → the progress list goes away; the AI summary replaced the fallback.
  await until(win, async () =>
    expect(await win.getByTestId('pipeline').count()).toBe(0),
  );
  await win.getByTestId('tab-summary').click();
  await expect(win.getByTestId('detail-summary')).toContainText('STUB-AI');
  await expect(win.getByTestId('llm-error')).toHaveCount(0);
  expect(errors).toEqual([]);
  await app.close();
  llm.server.close();
});

test('app restart mid post-processing resumes the pipeline from the persisted step', async () => {
  const userData = tmp();
  const llm = llmStub();
  const url = await llm.url();
  // Real backoff (scale 1): after the first 503 the summary waits ~2 s, so we can quit while it is retrying.
  const a = await launch(userData, {
    QRTZ_MOCK_SPEED: '0.02',
    QRTZ_SECRET_OPENROUTER: 'sk-test-stub',
    QRTZ_LLM_BASE_URL: url,
    QRTZ_RETRY_SCALE: '1',
  });
  await record(a.win);
  await until(a.win, async () =>
    expect(await a.win.getByTestId('live-segment').count()).toBeGreaterThan(3),
  );
  await a.win.getByTestId('memo-stop').click();
  await until(a.win, async () =>
    expect(
      await a.win
        .getByTestId('pipeline-step-summary')
        .getAttribute('data-status'),
    ).toBe('retrying'),
  );
  await expect(a.win.getByTestId('pipeline-step-finalize')).toHaveAttribute(
    'data-status',
    'done',
  );
  await a.app.close(); // quit between attempts
  const callsBefore = llm.state.calls;

  llm.state.fail = false;
  const b = await launch(userData, {
    QRTZ_SECRET_OPENROUTER: 'sk-test-stub',
    QRTZ_LLM_BASE_URL: url,
    QRTZ_MOCK_SCENARIO: '',
  });
  await b.win.getByTestId('nav-memo').click();
  await b.win.getByTestId('meeting-item').first().click();
  await until(b.win, async () =>
    expect(await b.win.getByTestId('pipeline').count()).toBe(0),
  );
  await b.win.getByTestId('tab-summary').click();
  await expect(b.win.getByTestId('detail-summary')).toContainText('STUB-AI');
  expect(llm.state.calls).toBe(callsBefore + 1); // resumed once; finalize was not redone
  await expect(b.win.getByTestId('meeting-item').first()).not.toContainText(
    /recording|processing|failed/,
  );
  expect(b.errors).toEqual([]);
  await b.app.close();
  llm.server.close();
});
