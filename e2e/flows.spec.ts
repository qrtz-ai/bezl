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
  await win.getByTestId('meeting-delete').click();
  await expect(first).toHaveCount(0);
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
