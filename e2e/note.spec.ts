import { _electron as electron, expect, test } from '@playwright/test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

// Full Note flow against the production build with the scripted mock AX helper:
// detect → record (fixture audio + ASR stream) → live transcript → stop → summary → list/search → export.
const appDir = path.join(__dirname, '..', 'apps', 'desktop');
const shots = path.join(__dirname, '..', 'test-results');

test('note end-to-end: detect, record, live transcript, summarize, search, export', async () => {
  const userData = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-note-'));
  const exportDir = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-export-'));
  const env: Record<string, string> = {
    ...(process.env as Record<string, string>),
    QRTZ_USER_DATA: userData,
    QRTZ_MOCK_HELPER: '1',
    QRTZ_MOCK_SCENARIO: 'standup',
    QRTZ_MOCK_SPEED: '0.04',
    QRTZ_EXPORT_DIR: exportDir,
    QRTZ_DEVTOOLS: '0',
  };
  delete env.QRTZ_SECRET_OPENROUTER; // prove the key-free path
  delete env.QRTZ_SECRET_LLM_OPENROUTER;
  const app = await electron.launch({
    args: [appDir, ...(process.platform === 'linux' ? ['--no-sandbox'] : [])],
    env,
  });
  const win = await app.firstWindow();
  const errors: string[] = [];
  win.on('pageerror', (e) => errors.push(e.message));

  await win.getByTestId('nav-note').click();

  // 1. Mock AX helper detects a Zoom meeting.
  const detected = win.getByTestId('note-detected');
  await expect(detected).toBeVisible({ timeout: 10_000 });
  await expect(detected).toContainText('Weekly Product Sync');
  await expect(detected).toContainText('Zoom');

  // 2. Record → live transcript streams in with AX speaker names.
  await win.getByTestId('note-record').click();
  const hud = win.getByTestId('note-hud');
  await expect(hud).toBeVisible();
  await expect(win.getByTestId('note-active-speaker')).toBeVisible({
    timeout: 10_000,
  });
  const liveView = win.getByTestId('live-transcript');
  await expect(liveView).toContainText('Grace Hopper', { timeout: 10_000 });
  await expect(liveView).toContainText(
    "let's ship the desktop launch next week",
    {
      timeout: 15_000,
    },
  );
  await win.screenshot({ path: path.join(shots, 'note-live.png') });

  // M6.3: the floating always-on-top mini HUD window mirrors the live session.
  await expect
    .poll(() => app.windows().some((w) => w.url().endsWith('#hud')), {
      timeout: 10_000,
    })
    .toBe(true);
  const mini = app.windows().find((w) => w.url().endsWith('#hud'))!;
  await expect(mini.getByTestId('mini-hud')).toBeVisible();
  await expect(mini.getByTestId('mini-hud-clock')).toHaveText(/\d\d:\d\d/);
  await expect(mini.getByTestId('mini-hud-line')).not.toHaveText(/^\s*$/, {
    timeout: 10_000,
  });
  expect(
    await app.evaluate(({ BrowserWindow }) =>
      BrowserWindow.getAllWindows().some((w) => w.isAlwaysOnTop()),
    ),
  ).toBe(true);
  await mini.screenshot({ path: path.join(shots, 'mini-hud.png') });

  // Mini HUD notes: expand, jot a note (stamped with the recording time), window grows.
  await mini.getByTestId('mini-hud-notes-toggle').click();
  await mini.getByTestId('mini-hud-note-input').fill('From the mini HUD');
  await mini.getByTestId('mini-hud-note-input').press('Enter');
  await expect(mini.getByTestId('mini-hud-note')).toContainText(
    /\d:\d\d\s*From the mini HUD/,
  );
  await expect
    .poll(() => mini.evaluate(() => window.innerHeight))
    .toBeGreaterThan(150);
  await expect(win.getByTestId('note')).toContainText('From the mini HUD');
  await mini.screenshot({ path: path.join(shots, 'mini-hud-notes.png') });
  await mini.getByTestId('mini-hud-notes-toggle').click();

  // Fixture audio was written for the recording.
  const recs = fs.readdirSync(path.join(userData, 'recordings'));
  expect(recs).toHaveLength(1);
  expect(
    fs.statSync(path.join(userData, 'recordings', recs[0]!)).size,
  ).toBeGreaterThan(1_000_000);

  // 3. Stop → summary + action items (extractive fallback, no keys).
  await win.getByTestId('note-stop').click();
  await expect(win.getByTestId('note-done')).toBeVisible({ timeout: 20_000 });
  await win.getByTestId('note-open').click();
  await win.getByTestId('tab-summary').click(); // transcript is the default tab
  const summary = win.getByTestId('detail-summary');
  await expect(summary).toBeVisible();
  const actions = win.getByTestId('action-item');
  await expect(actions.first()).toBeVisible();
  expect(await actions.count()).toBeGreaterThanOrEqual(3);
  await expect(summary).toContainText('onboarding copy');
  await expect(summary).toContainText('Grace Hopper');
  await expect(summary).toContainText('extractive');
  await win.screenshot({ path: path.join(shots, 'note-summary.png') });

  // Transcript tab: 8 segments, speaker-labelled.
  await win.getByTestId('tab-transcript').click();
  await expect(win.getByTestId('transcript-segment')).toHaveCount(8);
  await expect(win.getByTestId('detail-transcript')).toContainText(
    'Alan Turing',
  );

  // 4. Meeting list + full-text search.
  await expect(win.getByTestId('meeting-item')).toHaveCount(1);
  await win.getByTestId('note-search-toggle').click();
  await win.getByTestId('note-search').fill('parakeet');
  await expect(win.getByTestId('search-snippet')).toContainText('[parakeet]', {
    timeout: 5_000,
  });
  await win.getByTestId('note-search').fill('zebra');
  await expect(win.getByTestId('meeting-item')).toHaveCount(0);
  await win.getByTestId('note-search').fill('');
  await expect(win.getByTestId('meeting-item')).toHaveCount(1);

  // 5. Markdown export.
  await win.getByTestId('meeting-menu').click();
  await win.getByTestId('export-save').click();
  await expect(win.getByTestId('export-result')).toContainText('Saved to');
  const files = fs.readdirSync(exportDir);
  expect(files).toHaveLength(1);
  expect(files[0]).toMatch(/weekly-product-sync\.md$/);
  const md = fs.readFileSync(path.join(exportDir, files[0]!), 'utf8');
  expect(md).toContain('# Weekly Product Sync');
  expect(md).toContain('## Action items');
  expect(md).toContain('- [ ] ');
  expect(md).toContain('**[0:04] Grace Hopper:**');

  // Persistence: the meeting survives a restart.
  await app.close();
  const app2 = await electron.launch({
    args: [appDir, ...(process.platform === 'linux' ? ['--no-sandbox'] : [])],
    env: { ...env, QRTZ_MOCK_SCENARIO: '' },
  });
  const win2 = await app2.firstWindow();
  await expect(win2.getByTestId('nav-note')).toBeVisible();
  // The meeting list shows the recorded meeting.
  await expect(win2.getByTestId('meeting-item')).toHaveCount(1);
  await expect(win2.getByTestId('meeting-item')).toContainText(
    'Weekly Product Sync',
  );
  await win2.screenshot({ path: path.join(shots, 'note-list.png') });
  // ⌘K meeting search (FTS) opens the meeting in Note.
  await win2.keyboard.press('ControlOrMeta+k');
  await win2.getByTestId('palette-input').fill('onboarding');
  const hit = win2.locator('[data-testid^="cmd-meeting."]');
  await expect(hit).toHaveCount(1, { timeout: 5_000 });
  await expect(hit).toContainText('Weekly Product Sync');
  await hit.click();
  await expect(win2.getByTestId('meeting-item')).toHaveCount(1);
  await expect(win2.getByTestId('tab-transcript')).toBeVisible();
  await app2.close();

  expect(errors).toEqual([]);
});
