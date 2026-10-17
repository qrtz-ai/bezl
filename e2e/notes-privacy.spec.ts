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

// Live notes, privacy pause, summary templates, local-only mode, excluded apps, and backup/import.
const appDir = path.join(__dirname, '..', 'apps', 'desktop');
const shots = path.join(__dirname, '..', 'test-results');

async function launch(extra: Record<string, string>, userData?: string) {
  const env: Record<string, string> = {
    ...(process.env as Record<string, string>),
    QRTZ_USER_DATA:
      userData ?? fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-np-')),
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
  return { app, win, errors, userData: env.QRTZ_USER_DATA! };
}

async function until(win: Page, fn: () => Promise<void>, timeout = 20_000) {
  await expect(async () => {
    await win.evaluate(() => new Promise((r) => setTimeout(r, 50)));
    await fn();
  }).toPass({ timeout });
}

async function recordAndFinish(win: Page, during?: () => Promise<void>) {
  await win.getByTestId('nav-note').click();
  await expect(win.getByTestId('note-detected')).toBeVisible({
    timeout: 10_000,
  });
  await win.getByTestId('note-record').click();
  await expect(win.getByTestId('note-hud')).toBeVisible();
  if (during) await during();
  await until(win, async () => {
    expect(await win.getByTestId('live-segment').count()).toBeGreaterThan(1);
  });
  await win.getByTestId('note-stop').click();
  await until(win, async () => {
    expect(
      await win.getByTestId('meeting-item').first().innerText(),
    ).not.toMatch(/recording|processing/);
  });
  await win.getByTestId('meeting-item').first().click();
  await win.getByTestId('tab-summary').click(); // transcript is the default tab
  await expect(win.getByTestId('detail-summary')).toBeVisible({
    timeout: 15_000,
  });
}

test('live notes + privacy pause in the HUD, notes merged into the summary, templates re-run', async () => {
  const { app, win, errors } = await launch({
    QRTZ_MOCK_SCENARIO: 'standup',
    QRTZ_MOCK_SPEED: '0.3',
  });
  await recordAndFinish(win, async () => {
    // Pause straight away: nothing is transcribed while paused.
    await win.getByTestId('note-pause').click();
    await expect(win.getByTestId('note-paused')).toBeVisible();
    await win
      .getByTestId('note-input')
      .fill('Ask Grace about the launch checklist');
    await win.getByTestId('note-input').press('Enter');
    await expect(win.getByTestId('note')).toHaveCount(1);
    await expect(win.getByTestId('note-time').first()).toHaveText(/^0:0\d$/);
    await win.evaluate(() => new Promise((r) => setTimeout(r, 1500)));
    expect(await win.getByTestId('live-segment').count()).toBe(0);
    await win.getByTestId('note-pause').click();
    await expect(win.getByTestId('note-paused')).toHaveCount(0);
    await win.getByTestId('note-input').fill('TODO send recap to the team');
    await win.getByTestId('note-add').click();
    await expect(win.getByTestId('note')).toHaveCount(2);
  });
  await win.getByTestId('tab-summary').click(); // transcript is the default tab
  const summary = win.getByTestId('detail-summary');
  await expect(summary).toContainText(
    'Your notes: Ask Grace about the launch checklist; TODO send recap to the team.',
  );
  await expect(
    win
      .getByTestId('action-item')
      .filter({ hasText: 'send recap to the team' }),
  ).toHaveCount(1);
  await win.screenshot({ path: path.join(shots, 'notes-summary.png') });

  // Notes tab: edit a note; the change is saved.
  await win.getByRole('tab', { name: 'Notes' }).click();
  await win
    .getByRole('button', { name: /TODO send recap to the team \(edit note\)/ })
    .click();
  await win.getByTestId('note-edit').fill('TODO send recap to everyone');
  await win.getByTestId('note-edit').press('Enter');
  await expect(win.getByTestId('notes')).toContainText(
    'TODO send recap to everyone',
  );

  // Templates: pick "Standup", the summary re-runs and the choice is stored with the meeting.
  await win.getByRole('tab', { name: 'Summary' }).click();
  await expect(win.getByTestId('summary-template')).toHaveValue('general');
  await win.getByTestId('summary-template').selectOption('standup');
  await until(win, async () => {
    expect(
      await win.getByTestId('summary-rerun').getAttribute('aria-label'),
    ).toBe('Re-run');
  });
  await expect(summary).toContainText('send recap to everyone');
  await win.getByTestId('nav-settings').click();
  await win.getByTestId('nav-note').click();
  await win.getByTestId('meeting-item').first().click();
  await win.getByTestId('tab-summary').click();
  await expect(win.getByTestId('summary-template')).toHaveValue('standup');
  expect(errors).toEqual([]);
  await app.close();
});

test('privacy: local-only never calls the cloud LLM; excluded apps are not detected', async () => {
  let hits = 0;
  const server = http.createServer((req, res) => {
    hits++;
    req.resume();
    res.writeHead(500).end('{}');
  });
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  const port = (server.address() as AddressInfo).port;
  const { app, win, errors, userData } = await launch({
    QRTZ_MOCK_SCENARIO: 'standup',
    QRTZ_MOCK_SPEED: '0.02',
    QRTZ_SECRET_OPENROUTER: 'sk-test-stub',
    QRTZ_LLM_BASE_URL: `http://127.0.0.1:${port}/api/v1`,
  });
  await win.getByTestId('nav-settings').click();
  await win.getByTestId('local-only').click();
  await expect(win.getByTestId('local-only')).toHaveAttribute(
    'aria-checked',
    'true',
  );
  await win.getByTestId('pause-hotkey').fill('CommandOrControl+Alt+9');
  await win.getByTestId('pause-hotkey').blur();
  await win.getByTestId('excluded-apps').fill('Safari, webex');
  await win.getByTestId('excluded-apps').blur();
  await until(win, async () => {
    const cfg = JSON.parse(
      fs.readFileSync(path.join(userData, 'config.json'), 'utf8'),
    );
    expect(cfg.privacy).toEqual({
      localOnly: true,
      excludedApps: ['Safari', 'webex'],
      pauseHotkey: 'CommandOrControl+Alt+9',
    });
  });
  await win.screenshot({
    path: path.join(shots, 'settings-privacy.png'),
    fullPage: true,
  });
  await recordAndFinish(win);
  await win.getByTestId('tab-summary').click(); // transcript is the default tab
  await expect(win.getByTestId('detail-summary')).toContainText(
    'on-device extractive',
  );
  await expect(win.getByTestId('llm-error')).toHaveCount(0);
  expect(hits).toBe(0);
  // Chat stays on-device too.
  await win.getByRole('tab', { name: 'Ask' }).click();
  await win
    .getByRole('textbox')
    .last()
    .fill('What did we decide about the launch?');
  await win.getByRole('textbox').last().press('Enter');
  await win.evaluate(() => new Promise((r) => setTimeout(r, 1000)));
  expect(hits).toBe(0);
  expect(errors).toEqual([]);
  await app.close();
  server.close();

  // Excluded app: Zoom meetings are never detected (no prompt).
  const ud = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-np-ex-'));
  fs.writeFileSync(
    path.join(ud, 'config.json'),
    JSON.stringify({ privacy: { excludedApps: ['zoom'] } }),
  );
  const second = await launch(
    { QRTZ_MOCK_SCENARIO: 'standup', QRTZ_MOCK_SPEED: '0.02' },
    ud,
  );
  await second.win.getByTestId('nav-note').click();
  await second.win.evaluate(() => new Promise((r) => setTimeout(r, 2500)));
  await expect(second.win.getByTestId('note-detected')).toHaveCount(0);
  const log = fs.readFileSync(path.join(ud, 'logs', 'main.jsonl'), 'utf8');
  expect(log).toContain('meeting ignored (excluded app)');
  await second.app.close();
});

test('backup: export zip with checksums, import into a fresh profile, tampered zip rejected', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-bk-'));
  const zip = path.join(dir, 'qrtz-backup.zip');
  const a = await launch({
    QRTZ_MOCK_SCENARIO: 'standup',
    QRTZ_MOCK_SPEED: '0.02',
    QRTZ_BACKUP_PATH: zip,
  });
  await recordAndFinish(a.win, async () => {
    await a.win.getByTestId('note-input').fill('backup me');
    await a.win.getByTestId('note-input').press('Enter');
    await expect(a.win.getByTestId('note')).toHaveCount(1);
  });
  await a.win.getByTestId('nav-settings').click();
  await a.win.getByTestId('backup-export').click();
  await expect(a.win.getByTestId('backup-result')).toContainText(
    /Backed up 1 meetings and 1 recordings/,
    { timeout: 20_000 },
  );
  expect(fs.statSync(zip).size).toBeGreaterThan(10_000);
  await a.app.close();

  const b = await launch({ QRTZ_BACKUP_PATH: zip });
  await b.win.getByTestId('nav-settings').click();
  await b.win.getByTestId('backup-import').click();
  await expect(b.win.getByTestId('backup-result')).toContainText(
    'Verified and imported 1 meetings (0 already here, 1 recordings)',
    { timeout: 20_000 },
  );
  await b.win.getByTestId('backup-import').click();
  await expect(b.win.getByTestId('backup-result')).toContainText(
    'imported 0 meetings (1 already here',
  );
  await b.win.getByTestId('nav-note').click();
  await b.win.getByTestId('meeting-item').first().click();
  await b.win.getByTestId('tab-summary').click(); // transcript is the default tab
  await expect(b.win.getByTestId('detail-summary')).toContainText(
    'Your notes: backup me.',
  );
  expect(fs.readdirSync(path.join(b.userData, 'recordings')).length).toBe(1);
  expect(b.errors).toEqual([]);
  await b.app.close();

  // Flip one byte inside the stored audio: the integrity check rejects the whole backup.
  const buf = fs.readFileSync(zip);
  const tampered = path.join(dir, 'tampered.zip');
  const i = buf.indexOf(Buffer.from('data')) + 2000;
  buf[i] = buf[i]! ^ 0xff;
  fs.writeFileSync(tampered, buf);
  const c = await launch({ QRTZ_BACKUP_PATH: tampered });
  await c.win.getByTestId('nav-settings').click();
  await c.win.getByTestId('backup-import').click();
  await expect(c.win.getByTestId('backup-result')).toContainText(
    'Backup rejected',
    {
      timeout: 20_000,
    },
  );
  await c.app.close();
});
