import { _electron as electron, expect, test } from '@playwright/test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

// H5 + H6 against the built app and the mock helper: TCC revoked mid-capture degrades the recording live and stops
// a dictation gracefully; a dictation runs alongside a meeting recording (mux, shared mic tap).
const appDir = path.join(__dirname, '..', 'apps', 'desktop');
// Each launch gets its own user-data dir; removed after the file so e2e runs do not pile up temp dirs.
const userDataDirs: string[] = [];
test.afterAll(() => {
  for (const d of userDataDirs.splice(0))
    fs.rmSync(d, { recursive: true, force: true });
});

async function launch(extra: Record<string, string>) {
  const userData = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-perm-'));
  userDataDirs.push(userData);
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
  return { app, win, errors };
}

const meeting = {
  QRTZ_MOCK_SCENARIO: 'standup',
  QRTZ_MOCK_SPEED: '0.2',
};

test('H5: screen then mic revoked mid-recording → live mic-only, then no audio; recording keeps going', async () => {
  const { app, win, errors } = await launch({
    ...meeting,
    QRTZ_MOCK_REVOKE: 'screen-recording@800,microphone@2000',
  });
  await win.getByTestId('nav-note').click();
  await expect(win.getByTestId('note-detected')).toBeVisible({
    timeout: 10_000,
  });
  await win.getByTestId('note-record').click();
  await expect(win.getByTestId('note-stop')).toBeVisible();
  await expect(win.getByTestId('degraded-mic-only')).toBeVisible({
    timeout: 10_000,
  });
  await expect(win.getByTestId('degraded-audio-unavailable')).toBeVisible({
    timeout: 10_000,
  });
  await expect(win.getByTestId('note-stop')).toBeVisible();
  await win.getByTestId('note-stop').click();
  await expect(win.getByTestId('note-stop')).toBeHidden({ timeout: 10_000 });
  expect(errors).toEqual([]);
  await app.close();
});

test('H6: dictation runs during a meeting recording (mux); the recording is untouched', async () => {
  const { app, win, errors } = await launch(meeting);
  await win.getByTestId('nav-note').click();
  await expect(win.getByTestId('note-detected')).toBeVisible({
    timeout: 10_000,
  });
  await win.getByTestId('note-record').click();
  await expect(win.getByTestId('note-stop')).toBeVisible();
  await win.getByTestId('nav-talk').click();
  await win.getByTestId('talk-toggle').click();
  await expect(
    win.getByTestId('talk-live').getByTestId('talk-pill-text'),
  ).toContainText('friday', { timeout: 10_000 });
  await win.getByTestId('talk-toggle').click();
  await expect(win.getByTestId('talk-entry').first()).toBeVisible({
    timeout: 10_000,
  });
  await win.getByTestId('nav-note').click();
  await expect(win.getByTestId('note-stop')).toBeVisible();
  await expect(win.getByTestId('live-transcript')).not.toBeEmpty();
  await win.getByTestId('note-stop').click();
  await expect(win.getByTestId('note-stop')).toBeHidden({ timeout: 10_000 });
  expect(errors).toEqual([]);
  await app.close();
});

test('H5: microphone revoked mid-dictation stops gracefully: text kept, reason shown', async () => {
  const { app, win, errors } = await launch({
    QRTZ_MOCK_REVOKE: 'microphone@400',
  });
  await win.getByTestId('nav-talk').click();
  await win.getByTestId('talk-toggle').click();
  await expect(win.getByTestId('talk-live')).toContainText(
    /microphone access was turned off/,
    {
      timeout: 10_000,
    },
  );
  await expect(win.getByTestId('talk-entry').first()).toBeVisible();
  expect(errors).toEqual([]);
  await app.close();
});

test('N29/N32: a silent capture source shows live and clears when it is audible again; all silent shows "No sound"', async () => {
  const { app, win, errors } = await launch({
    ...meeting,
    QRTZ_MOCK_SILENT: 'mic@600-2500',
  });
  await win.getByTestId('nav-note').click();
  await expect(win.getByTestId('note-detected')).toBeVisible({
    timeout: 10_000,
  });
  await win.getByTestId('note-record').click();
  await expect(win.getByTestId('degraded-silent-mic')).toBeVisible({
    timeout: 10_000,
  });
  await expect(win.getByTestId('degraded-silent-mic')).toBeHidden({
    timeout: 10_000,
  });
  await win.getByTestId('note-stop').click();
  await expect(win.getByTestId('note-stop')).toBeHidden({ timeout: 10_000 });
  expect(errors).toEqual([]);
  await app.close();

  const second = await launch({
    ...meeting,
    QRTZ_MOCK_SILENT: 'system@600,mic@600',
  });
  await second.win.getByTestId('nav-note').click();
  await expect(second.win.getByTestId('note-detected')).toBeVisible({
    timeout: 10_000,
  });
  await second.win.getByTestId('note-record').click();
  await expect(second.win.getByTestId('degraded-no-sound')).toBeVisible({
    timeout: 10_000,
  });
  await expect(second.win.getByTestId('degraded-silent-mic')).toHaveCount(0);
  await second.win.getByTestId('note-stop').click();
  await expect(second.win.getByTestId('note-stop')).toBeHidden({
    timeout: 10_000,
  });
  expect(second.errors).toEqual([]);
  await second.app.close();
});
