import {
  _electron as electron,
  expect,
  test,
  type ElectronApplication,
  type Page,
} from '@playwright/test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

// qrtz talk end-to-end against the built app with the mock helper streaming scripted dictation partials/finals.
const appDir = path.join(__dirname, '..', 'apps', 'desktop');
const shots = path.join(__dirname, '..', 'test-results');

async function launch(extra: Record<string, string> = {}) {
  const userData = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-talk-'));
  const env: Record<string, string> = {
    ...(process.env as Record<string, string>),
    QRTZ_USER_DATA: userData,
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
  await win.getByTestId('nav-talk').click();
  await expect(win.getByTestId('talk')).toBeVisible();
  return { app, win, userData, errors };
}

const stored = (userData: string) =>
  JSON.parse(fs.readFileSync(path.join(userData, 'talk.json'), 'utf8')) as {
    history: {
      cleanup: string;
      insert: string;
      engine: string;
      failover: boolean;
      latency: Record<string, number>;
    }[];
  };

async function dictate(win: Page) {
  await win.getByTestId('talk-toggle').click();
  const text = win.getByTestId('talk-live').getByTestId('talk-pill-text');
  await expect(text).toContainText('friday', { timeout: 10_000 });
  await win.getByTestId('talk-toggle').click();
  await expect(win.getByTestId('talk-entry').first()).toBeVisible({
    timeout: 10_000,
  });
}

async function close(app: ElectronApplication, errors: string[]) {
  expect(errors).toEqual([]);
  await app.close();
}

test('talk: live streaming text, pill window, cleanup + dictionary, insert, history, latency', async () => {
  const { app, win, userData, errors } = await launch({
    QRTZ_TALK_LLM: 'fake',
  });
  await expect(win.getByTestId('nav-talk')).toHaveText('talk');
  await win.getByTestId('talk-term-input').fill('qrtz = cue arts');
  await win.getByTestId('talk-term-add').click();
  await expect(win.getByTestId('talk-term')).toContainText('qrtz');

  await win.getByTestId('talk-toggle').click();
  const text = win.getByTestId('talk-live').getByTestId('talk-pill-text');
  // Text streams in a bit at a time: an early snapshot is a strict prefix of a later one.
  await expect(text).not.toHaveText(/^…?$/, { timeout: 5000 });
  const early = (await text.textContent()) ?? '';
  await expect(text).toContainText('friday', { timeout: 10_000 });
  const later = (await text.textContent()) ?? '';
  expect(later.length).toBeGreaterThan(early.replace('…', '').length);
  await expect(win.getByTestId('talk-level')).toBeVisible();
  await expect
    .poll(() => app.windows().some((w) => w.url().endsWith('#pill')), {
      timeout: 5000,
    })
    .toBe(true);
  const pill = app.windows().find((w) => w.url().endsWith('#pill'))!;
  await expect(pill.getByTestId('talk-pill-text')).toContainText('friday');
  await win.screenshot({ path: path.join(shots, 'talk-live.png') });

  await win.getByTestId('talk-toggle').click();
  const entry = win.getByTestId('talk-entry').first();
  await expect(entry.getByTestId('talk-entry-text')).toHaveText(
    'I think we should ship qrtz talk on friday.',
    {
      timeout: 10_000,
    },
  );
  await expect(entry.getByTestId('talk-entry-meta')).toContainText(
    'Mock Editor',
  );
  await expect(entry.getByTestId('talk-entry-meta')).toContainText(/\d+ ms/);
  const h = stored(userData).history[0]!;
  expect(h).toMatchObject({
    cleanup: 'llm',
    insert: 'ax-selected-text',
    engine: 'on-device',
  });
  expect(h.latency.firstPartialMs).toBeGreaterThan(0);
  expect(h.latency.insertedMs).toBeGreaterThanOrEqual(0);
  await win.screenshot({ path: path.join(shots, 'talk-history.png') });

  // Copy puts the text on the clipboard; delete removes it.
  await entry.getByTestId('talk-entry-copy').click();
  await expect
    .poll(() => app.evaluate(({ clipboard }) => clipboard.readText()))
    .toBe('I think we should ship qrtz talk on friday.');
  await entry.getByTestId('talk-entry-delete').click();
  await expect(win.getByTestId('talk-entry')).toHaveCount(0);
  await close(app, errors);
});

test('talk: cleanup timeout falls back to on-device rules', async () => {
  const { app, win, userData, errors } = await launch({
    QRTZ_TALK_LLM: 'hang',
  });
  await dictate(win);
  await expect(win.getByTestId('talk-entry-text').first()).toHaveText(
    'So I think we should ship cue arts talk on friday.',
  );
  const h = stored(userData).history[0]!;
  expect(h.cleanup).toBe('rules');
  expect(h.latency.cleanupMs).toBeGreaterThanOrEqual(800);
  await close(app, errors);
});

test('talk: insertion falls back to paste, and to the clipboard when nothing can insert', async () => {
  const paste = await launch({ QRTZ_MOCK_INSERT: 'paste' });
  await dictate(paste.win);
  expect(stored(paste.userData).history[0]).toMatchObject({ insert: 'paste' });
  await close(paste.app, paste.errors);

  const fail = await launch({ QRTZ_MOCK_INSERT: 'fail' });
  await dictate(fail.win);
  await expect(fail.win.getByTestId('talk-entry-meta').first()).toContainText(
    'copied to clipboard',
  );
  await expect
    .poll(() => fail.app.evaluate(({ clipboard }) => clipboard.readText()))
    .toBe('So I think we should ship cue arts talk on friday.');
  await close(fail.app, fail.errors);
});

test('talk: cloud engine primary, automatic failover to on-device when the stream dies', async () => {
  const ok = await launch({ QRTZ_TALK_CLOUD: 'echo' });
  await dictate(ok.win);
  await expect(ok.win.getByTestId('talk-entry-meta').first()).toContainText(
    'deepgram',
  );
  expect(stored(ok.userData).history[0]).toMatchObject({
    engine: 'deepgram',
    failover: false,
  });
  await close(ok.app, ok.errors);

  const bad = await launch({ QRTZ_TALK_CLOUD: 'fail' });
  await dictate(bad.win);
  await expect(bad.win.getByTestId('talk-entry-meta').first()).toContainText(
    'on-device (fallback)',
  );
  await close(bad.app, bad.errors);
});

test('talk: settings has a toggle for every automatic behavior', async () => {
  const { app, win, errors } = await launch();
  await win.getByTestId('nav-settings').click();
  const s = win.getByTestId('talk-settings');
  for (const id of [
    'talk-enabled',
    'talk-auto-failover',
    'talk-warm-mic',
    'talk-auto-cleanup',
    'talk-ai-cleanup',
    'talk-auto-insert',
    'talk-restore-clipboard',
    'talk-show-pill',
    'talk-save-history',
  ])
    await expect(s.getByTestId(id)).toBeVisible();
  await s.getByTestId('talk-engine').selectOption('deepgram');
  await expect(s.getByTestId('talk-engine-key')).toBeVisible();
  await s.getByTestId('talk-show-pill').click();
  await win.getByTestId('nav-talk').click();
  await win.getByTestId('talk-toggle').click();
  await expect(
    win.getByTestId('talk-live').getByTestId('talk-pill-text'),
  ).toContainText('so', { timeout: 5000 });
  expect(app.windows().some((w) => w.url().endsWith('#pill'))).toBe(false);
  await win.getByTestId('talk-toggle').click();
  await close(app, errors);
});
