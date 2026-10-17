import { axe } from './axe';
import { _electron as electron, expect, test } from '@playwright/test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const appDir = path.join(__dirname, '..', 'apps', 'desktop');

async function launch(userData: string, extra: Record<string, string> = {}) {
  const app = await electron.launch({
    args: [appDir, ...(process.platform === 'linux' ? ['--no-sandbox'] : [])],
    env: {
      ...(process.env as Record<string, string>),
      QRTZ_USER_DATA: userData,
      QRTZ_MOCK_HELPER: '1',
      QRTZ_DEVTOOLS: '0',
      ...extra,
    },
  });
  const win = await app.firstWindow();
  await expect(win.getByTestId('home')).toBeVisible();
  return { app, win };
}

test('delete a meeting and copy its Markdown', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-flows-'));
  const { app, win } = await launch(dir, { QRTZ_DEV_SEED: 'many' });
  await win.getByTestId('nav-memo').click();
  const first = win
    .getByTestId('meeting-item')
    .filter({ hasText: 'Sync #500' });
  await expect(first).toBeVisible({ timeout: 15_000 });
  await first.click();
  await win.getByTestId('export-copy').click();
  await expect(win.getByTestId('export-result')).toBeVisible();
  const clip = await app.evaluate(({ clipboard }) => clipboard.readText());
  expect(clip).toMatch(/^# Sync \\?#500/);
  // Confirm dialog: Cancel keeps the meeting.
  await win.getByTestId('meeting-delete').click();
  await expect(win.getByTestId('confirm-delete')).toContainText('Sync #500');
  await win.getByTestId('confirm-delete-cancel').click();
  await expect(win.getByTestId('confirm-delete')).toHaveCount(0);
  await expect(first).toBeVisible();
  // Delete → hidden at once, Undo in the toast restores it.
  await win.getByTestId('meeting-delete').click();
  await win.getByTestId('confirm-delete-ok').click();
  await expect(first).toHaveCount(0);
  await expect(win.getByTestId('toast')).toContainText('Deleted “Sync #500”');
  await win.getByTestId('toast-action').click();
  await expect(first).toBeVisible();
  await expect(win.getByTestId('meeting-detail')).toContainText('Sync #500');
  // Delete for real: once the undo window passes it's gone from the database.
  await win.getByTestId('meeting-delete').click();
  await win.getByTestId('confirm-delete-ok').click();
  await expect(first).toHaveCount(0);
  await expect(win.getByTestId('toast')).toHaveCount(0, { timeout: 10_000 });
  await win.waitForTimeout(500);
  await expect(win.getByTestId('meeting-item').first()).toContainText(
    'Sync #499',
  );
  await app.close();
  // Deletion is durable.
  const again = await launch(dir);
  await again.win.getByTestId('nav-memo').click();
  await expect(again.win.getByTestId('meeting-item').first()).toContainText(
    'Sync #499',
  );
  await again.app.close();
});

test('settings persist across restarts; API key save and remove', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-flows-'));
  const { app, win } = await launch(dir);
  await win.getByTestId('nav-settings').click();
  await win.getByTestId('retain-audio').selectOption('30');
  await win.getByTestId('default-template').selectOption('standup');
  await win.getByTestId('theme').selectOption('light');
  await win.getByTestId('llm-key').fill('sk-test-not-real');
  await win.getByRole('button', { name: 'Save', exact: true }).click();
  // Linux CI has no keyring, so safeStorage refuses and the key must not be stored in plaintext.
  const secure = process.platform === 'darwin';
  if (secure) await expect(win.getByText('Key configured')).toBeVisible();
  else
    await expect(win.getByRole('alert')).toContainText(
      'Secure storage is not available',
    );
  await app.close();

  const again = await launch(dir);
  await expect(again.win.locator('html')).toHaveAttribute(
    'data-theme',
    'light',
  );
  await again.win.getByTestId('nav-settings').click();
  await expect(again.win.getByTestId('retain-audio')).toHaveValue('30');
  await expect(again.win.getByTestId('default-template')).toHaveValue(
    'standup',
  );
  await expect(
    again.win.getByText(
      secure ? 'Key configured' : 'Using extractive fallback',
    ),
  ).toBeVisible();
  // The key never reaches the renderer or the config file.
  await expect(again.win.getByTestId('llm-key')).toHaveValue('');
  const files = fs.readdirSync(dir, { recursive: true }) as string[];
  for (const f of files) {
    const p = path.join(dir, f);
    const st = fs.lstatSync(p, { throwIfNoEntry: false });
    if (st?.isFile() && st.size < 5e6)
      expect(fs.readFileSync(p, 'utf8'), f).not.toContain('sk-test-not-real');
  }
  if (secure) {
    await again.win.getByRole('button', { name: 'Remove' }).click();
    await expect(
      again.win.getByText('Using extractive fallback'),
    ).toBeVisible();
  }
  await again.app.close();
});

test('mini HUD follows the light theme', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-flows-'));
  const { app, win } = await launch(dir, {
    QRTZ_MOCK_SCENARIO: 'standup',
    QRTZ_MOCK_SPEED: '0.04',
  });
  await win.getByTestId('nav-settings').click();
  await win.getByTestId('theme').selectOption('light');
  await win.getByTestId('nav-memo').click();
  await expect(win.getByTestId('memo-detected')).toBeVisible({
    timeout: 10_000,
  });
  await win.getByTestId('memo-record').click();
  await expect
    .poll(() => app.windows().some((w) => w.url().endsWith('#hud')), {
      timeout: 10_000,
    })
    .toBe(true);
  const mini = app.windows().find((w) => w.url().endsWith('#hud'))!;
  await expect(mini.getByTestId('mini-hud')).toBeVisible();
  await expect(mini.locator('html')).toHaveAttribute('data-theme', 'light');
  await mini.getByTestId('mini-hud-notes-toggle').click();
  await expect(mini.getByTestId('mini-hud-notes')).toBeVisible();
  await mini.waitForTimeout(400);
  fs.mkdirSync(path.join(__dirname, '..', 'test-results', 'themes'), {
    recursive: true,
  });
  await mini.screenshot({
    path: path.join(
      __dirname,
      '..',
      'test-results',
      'themes',
      'light-mini-hud.png',
    ),
  });
  const bad = await axe(mini, 'light/mini-hud');
  expect(bad).toEqual([]);
  await app.close();
});

test('backup progress bar shows on a large seeded recording', async () => {
  test.setTimeout(120_000);
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-flows-'));
  const rec = path.join(dir, 'recordings');
  fs.mkdirSync(rec, { recursive: true });
  const fd = fs.openSync(path.join(rec, 'big-seed.wav'), 'w');
  const chunk = Buffer.alloc(8 * 1024 * 1024, 5);
  for (let i = 0; i < 48; i++) fs.writeSync(fd, chunk); // ~400 MB
  fs.closeSync(fd);
  const out = path.join(dir, 'out', 'backup.zip');
  const { app, win } = await launch(dir, { QRTZ_BACKUP_PATH: out });
  await win.getByTestId('nav-settings').click();
  await win.getByTestId('backup-export').click();
  const bar = win.getByTestId('backup-progress');
  await expect(bar).toBeVisible({ timeout: 10_000 });
  await expect(bar.getByRole('progressbar')).toHaveAttribute(
    'aria-valuenow',
    /\d+/,
  );
  await expect(bar).toContainText(/Computing checksums|Writing zip/);
  await win.screenshot({
    path: path.join(__dirname, '..', 'test-results', 'backup-progress.png'),
  });
  await expect(win.getByTestId('backup-result')).toContainText('Backed up', {
    timeout: 90_000,
  });
  await expect(bar).toHaveCount(0);
  expect(fs.statSync(out).size).toBeGreaterThan(400e6);
  await app.close();
});

test('quitting inside the undo window still deletes (tombstone completed on next launch)', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-flows-'));
  const { app, win } = await launch(dir, { QRTZ_DEV_SEED: 'many' });
  await win.getByTestId('nav-memo').click();
  const item = win.getByTestId('meeting-item').filter({ hasText: 'Sync #500' });
  await expect(item).toBeVisible({ timeout: 15_000 });
  await item.click();
  await win.getByTestId('meeting-delete').click();
  await win.getByTestId('confirm-delete-ok').click();
  await expect(win.getByTestId('toast')).toBeVisible();
  const tomb = path.join(dir, 'pending-deletes.json');
  const id = (JSON.parse(fs.readFileSync(tomb, 'utf8')) as string[])[0];
  expect(id).toBeTruthy();
  await app.close(); // well inside the 6 s undo window
  expect(JSON.parse(fs.readFileSync(tomb, 'utf8'))).toEqual([id]);

  const again = await launch(dir);
  await expect
    .poll(() => JSON.parse(fs.readFileSync(tomb, 'utf8')) as string[], {
      timeout: 10_000,
    })
    .toEqual([]);
  await again.win.getByTestId('nav-memo').click();
  await expect(again.win.getByTestId('meeting-item').first()).toContainText(
    'Sync #499',
  );
  await expect(
    again.win.getByTestId('meeting-item').filter({ hasText: 'Sync #500' }),
  ).toHaveCount(0);
  await again.app.close();
});
