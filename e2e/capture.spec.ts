import { _electron as electron, expect, test } from '@playwright/test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

// N29 against the built app and the mock helper (QRTZ_MOCK_SILENT): a silent source shows a live badge that clears
// when audio returns; every source silent shows the stronger no-audio badge, and the finished meeting keeps a notice.
const appDir = path.join(__dirname, '..', 'apps', 'desktop');

async function launch(extra: Record<string, string>) {
  const env: Record<string, string> = {
    ...(process.env as Record<string, string>),
    QRTZ_USER_DATA: fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-capture-')),
    QRTZ_MOCK_HELPER: '1',
    QRTZ_DEVTOOLS: '0',
    QRTZ_MOCK_SCENARIO: 'standup',
    QRTZ_MOCK_SPEED: '0.2',
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
  await win.getByTestId('nav-note').click();
  await expect(win.getByTestId('note-detected')).toBeVisible({
    timeout: 10_000,
  });
  await win.getByTestId('note-record').click();
  await expect(win.getByTestId('note-stop')).toBeVisible();
  return { app, win, errors };
}

test('N29: system audio goes silent → badge with a time of day, then clears when audio returns', async () => {
  const { app, win, errors } = await launch({
    QRTZ_MOCK_SILENT: 'system@1000-3500',
  });
  const badge = win.getByTestId('capture-silent-system');
  await expect(badge).toBeVisible({ timeout: 10_000 });
  await expect(badge).toHaveText(
    /^no audio from system audio since \d\d:\d\d$/,
  );
  await expect(win.getByTestId('capture-no-audio')).toBeHidden();
  await expect(badge).toBeHidden({ timeout: 10_000 });
  await expect(win.getByTestId('note-stop')).toBeVisible();
  await win.getByTestId('note-stop').click();
  await expect(win.getByTestId('note-stop')).toBeHidden({ timeout: 10_000 });
  expect(errors).toEqual([]);
  await app.close();
});

test('N29: every source silent → "no audio is being recorded"; the meeting keeps a capture notice', async () => {
  const { app, win, errors } = await launch({
    QRTZ_MOCK_SILENT: 'system@800,mic@800',
  });
  await expect(win.getByTestId('capture-no-audio')).toHaveText(
    'no audio is being recorded',
    { timeout: 10_000 },
  );
  await expect(win.getByTestId('capture-silent-mic')).toBeHidden();
  await win.waitForTimeout(500);
  await win.getByTestId('note-stop').click();
  await expect(win.getByTestId('note-done')).toBeVisible({ timeout: 20_000 });
  await win.getByTestId('note-open').click();
  const notice = win.getByTestId('capture-notice');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toContainText('no audio from system audio');
  await expect(notice).toContainText('no audio from mic');
  expect(errors).toEqual([]);
  await app.close();
});
