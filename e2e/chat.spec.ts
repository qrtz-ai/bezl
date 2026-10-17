import {
  _electron as electron,
  expect,
  test,
  type Page,
} from '@playwright/test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { axe } from './axe';

// Chat (per-meeting + across meetings, key-free extractive path), speaker rename/merge, auto-record prompt and
// "always auto-record", and search-result paging.
const appDir = path.join(__dirname, '..', 'apps', 'desktop');
const shots = path.join(__dirname, '..', 'test-results');

async function launch(extra: Record<string, string>, userData?: string) {
  const env: Record<string, string> = {
    ...(process.env as Record<string, string>),
    QRTZ_USER_DATA:
      userData ?? fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-chat-')),
    QRTZ_MOCK_HELPER: '1',
    QRTZ_DEVTOOLS: '0',
    ...extra,
  };
  delete env.QRTZ_SECRET_OPENROUTER;
  delete env.QRTZ_SECRET_LLM_OPENROUTER;
  const app = await electron.launch({
    args: [appDir, ...(process.platform === 'linux' ? ['--no-sandbox'] : [])],
    env,
  });
  const win = await app.firstWindow();
  const errors: string[] = [];
  win.on('pageerror', (e) => errors.push(e.message));
  return { app, win, errors };
}

/** xvfb windows count as occluded (throttled timers), so poke the page while polling. */
async function until(win: Page, fn: () => Promise<void>, timeout = 15_000) {
  await expect(async () => {
    await win.evaluate(() => new Promise((r) => setTimeout(r, 50)));
    await fn();
  }).toPass({ timeout });
}

async function recordStandup(win: Page) {
  await win.getByTestId('nav-note').click();
  await expect(win.getByTestId('note-detected')).toBeVisible({
    timeout: 10_000,
  });
  await win.getByTestId('note-record').click();
  await until(win, async () => {
    expect(await win.getByTestId('live-segment').count()).toBeGreaterThan(5);
  });
  await win.getByTestId('note-stop').click();
  await expect(win.getByTestId('note-done')).toBeVisible({ timeout: 20_000 });
  await win.getByTestId('note-open').click();
  await win.getByTestId('tab-summary').click(); // transcript is the default tab
  await expect(win.getByTestId('detail-summary')).toBeVisible();
}

test('chat per meeting and across meetings, speaker rename + merge', async () => {
  const { app, win, errors } = await launch({
    QRTZ_MOCK_SCENARIO: 'standup',
    QRTZ_MOCK_SPEED: '0.02',
    // Semantic retrieval on (shared model cache). If the model can't load, chat falls back to full-text and still passes.
    QRTZ_EMBEDDINGS: '1',
    QRTZ_MODELS_DIR: path.join(os.tmpdir(), 'qrtz-e2e-models'),
  });
  await recordStandup(win);

  // Speaker rename, then merge.
  const chips = win.getByTestId('speaker-chip');
  await expect(chips.first()).toBeVisible();
  const n = await chips.count();
  expect(n).toBeGreaterThan(1);
  const first = (await chips.first().innerText()).split(' · ')[0]!.trim();
  await chips.first().click();
  await win.getByTestId('speaker-name').fill('Ada (PM)');
  await win.getByTestId('speaker-save').click();
  await until(win, async () => {
    expect(await win.getByTestId('speakers').innerText()).toContain('Ada (PM)');
  });
  expect(await win.getByTestId('speakers').innerText()).not.toContain(first);
  await win
    .getByTestId('speaker-chip')
    .filter({ hasNotText: 'Ada (PM)' })
    .first()
    .click();
  await win.getByTestId('speaker-merge').selectOption('Ada (PM)');
  await until(win, async () => {
    expect(await win.getByTestId('speaker-chip').count()).toBe(n - 1);
  });
  await win.getByTestId('tab-transcript').click();
  await expect(
    win
      .getByTestId('transcript-segment')
      .filter({ hasText: 'Ada (PM)' })
      .first(),
  ).toBeVisible();

  // Per-meeting Q&A (no key: extractive quotes with citations).
  await win.getByTestId('tab-ask').click();
  await win
    .getByTestId('chat-input')
    .fill('Who will write the onboarding copy?');
  await win.getByTestId('chat-send').click();
  await until(win, async () => {
    expect(await win.getByTestId('chat-answer').innerText()).toMatch(
      /onboarding/i,
    );
  });
  await expect(win.getByTestId('chat-source').first()).toBeVisible();
  await win.screenshot({ path: path.join(shots, 'chat-meeting.png') });
  const a11y = await axe(win, 'chat-meeting');
  await win.getByTestId('speaker-chip').first().click(); // open the speaker editor
  a11y.push(...(await axe(win, 'speaker-editor')));
  await win.getByTestId('speaker-chip').first().click();
  await win.getByTestId('chat-source').first().click();
  await expect(win.getByTestId('detail-transcript')).toBeVisible();
  await expect(
    win.locator('[data-testid="transcript-segment"][data-active]'),
  ).toHaveCount(1);

  // Across all meetings, citation opens the meeting.
  await win.getByTestId('note-ask-all').click();
  await win.getByTestId('chat-input').fill('parakeet latency');
  await win.getByTestId('chat-send').click();
  await until(win, async () => {
    expect(await win.getByTestId('chat-answer').innerText()).toMatch(
      /parakeet/i,
    );
  });
  await win.screenshot({ path: path.join(shots, 'chat-all.png') });
  a11y.push(...(await axe(win, 'chat-all')));
  expect(a11y).toEqual([]);
  await win.getByTestId('chat-source').first().click();
  await expect(win.getByTestId('meeting-detail')).toBeVisible();
  await expect(win.getByTestId('detail-transcript')).toBeVisible();
  expect(errors).toEqual([]);
  await app.close();
});

test('detected meeting prompts; "Always record" persists and auto-starts next time', async () => {
  const userData = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-auto-'));
  const env = { QRTZ_MOCK_SCENARIO: 'standup', QRTZ_MOCK_SPEED: '0.05' };
  let { app, win } = await launch(env, userData);
  await win.getByTestId('nav-note').click();
  await expect(win.getByTestId('note-detected')).toBeVisible({
    timeout: 10_000,
  });
  await win.getByTestId('note-always-record').click();
  await expect(win.getByTestId('note-hud')).toBeVisible();
  await win.getByTestId('note-stop').click();
  await app.close();
  const cfg = JSON.parse(
    fs.readFileSync(path.join(userData, 'config.json'), 'utf8'),
  );
  expect(cfg.note.autoRecord).toBe(true);

  ({ app, win } = await launch(env, userData));
  await win.getByTestId('nav-note').click();
  await expect(win.getByTestId('note-hud')).toBeVisible({ timeout: 10_000 }); // no prompt, recording started
  await expect(win.getByTestId('note-detected')).toHaveCount(0);
  await win.getByTestId('nav-settings').click();
  await expect(win.getByTestId('auto-record')).toHaveAttribute(
    'aria-checked',
    'true',
  );
  await win.getByTestId('auto-record').click();
  await expect(win.getByTestId('auto-record')).toHaveAttribute(
    'aria-checked',
    'false',
  );
  await app.close();
  expect(
    JSON.parse(fs.readFileSync(path.join(userData, 'config.json'), 'utf8')).note
      .autoRecord,
  ).toBe(false);
});

test('search results are paged', async () => {
  const { app, win, errors } = await launch({ QRTZ_DEV_SEED: 'many' });
  await win.getByTestId('nav-note').click();
  await until(win, async () => {
    expect(await win.getByTestId('meeting-item').count()).toBe(100);
  });
  await win.getByTestId('note-search-toggle').click();
  await win.getByTestId('note-search').fill('roadmap');
  await until(win, async () => {
    expect(await win.getByTestId('search-snippet').count()).toBe(100);
  });
  await win.getByTestId('note-show-more').click();
  await until(win, async () => {
    expect(await win.getByTestId('search-snippet').count()).toBe(200);
  });
  expect(errors).toEqual([]);
  await app.close();
});
