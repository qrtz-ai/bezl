import { _electron as electron, expect, test } from '@playwright/test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

// Fail-soft: the native helper dies mid-meeting. The supervisor restarts it and the recording resumes into a
// second WAV part (docs/recording-lifecycle.md F3); at stop, the parts are stitched + header-repaired and the
// meeting gets a summary. Nothing is lost from before the crash and the session never sticks in "recording".
const appDir = path.join(__dirname, '..', 'apps', 'desktop');
const shots = path.join(__dirname, '..', 'test-results');

test('helper crash mid-recording: reconnects, resumes, stitches audio, keeps transcript', async () => {
  const userData = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-crash-'));
  const env: Record<string, string> = {
    ...(process.env as Record<string, string>),
    QRTZ_USER_DATA: userData,
    QRTZ_MOCK_HELPER: '1',
    QRTZ_MOCK_SCENARIO: 'standup',
    QRTZ_MOCK_SPEED: '0.04',
    QRTZ_MOCK_CRASH_AFTER_MS: '900',
    QRTZ_DEVTOOLS: '0',
  };
  delete env.QRTZ_SECRET_OPENROUTER;
  const app = await electron.launch({
    args: [appDir, ...(process.platform === 'linux' ? ['--no-sandbox'] : [])],
    env,
  });
  const win = await app.firstWindow();
  const errors: string[] = [];
  win.on('pageerror', (e) => errors.push(e.message));
  await win.getByTestId('nav-memo').click();
  await expect(win.getByTestId('memo-detected')).toBeVisible({
    timeout: 10_000,
  });
  await win.getByTestId('memo-record').click();
  await expect(win.getByTestId('memo-hud')).toBeVisible();

  // Helper dies ~0.9 s in, comes back, and capture resumes; then the user stops.
  await expect
    .poll(
      async () =>
        (await app.evaluate(
          () => (globalThis as { __qrtzPhases?: string[] }).__qrtzPhases ?? [],
        )) as string[],
      { timeout: 15_000 },
    )
    .toContain('reconnecting');
  await expect(win.getByTestId('memo-stop')).toBeVisible({ timeout: 15_000 });
  await win.getByTestId('memo-stop').click();
  await expect(win.getByTestId('meeting-item').first()).not.toContainText(
    'recording',
    {
      timeout: 15_000,
    },
  );
  await win.getByTestId('meeting-item').first().click();
  await expect(win.getByTestId('detail-summary')).toBeVisible({
    timeout: 15_000,
  });
  await win.getByTestId('tab-transcript').click();
  const segs = await win.getByTestId('transcript-segment').count();
  expect(segs).toBeGreaterThan(0); // captured before the crash, plus whatever came after the resume
  await win.screenshot({ path: path.join(shots, 'crash-salvaged.png') });

  const recs = fs.readdirSync(path.join(userData, 'recordings'));
  expect(recs).toHaveLength(1); // the resumed part was stitched into the first and removed
  const wav = fs.readFileSync(path.join(userData, 'recordings', recs[0]!));
  expect(wav.readUInt32LE(40)).toBe(wav.length - 44); // header repaired
  expect(wav.length).toBeGreaterThan(100_000);

  expect(errors).toEqual([]);
  await app.close();
});
