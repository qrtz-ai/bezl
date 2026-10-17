import { axe } from './axe';
import { _electron as electron, expect, test } from '@playwright/test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const appDir = path.join(__dirname, '..', 'apps', 'desktop');
const shots = path.join(__dirname, '..', 'test-results', 'themes');

/** Every view in both themes: screenshot + axe (serious/critical) with real data on screen. */
for (const mode of ['dark', 'light'] as const) {
  test(`${mode} theme: every view renders, is screenshotted and axe-clean`, async () => {
    test.setTimeout(120_000);
    fs.mkdirSync(shots, { recursive: true });
    const app = await electron.launch({
      args: [appDir, ...(process.platform === 'linux' ? ['--no-sandbox'] : [])],
      env: {
        ...(process.env as Record<string, string>),
        QRTZ_USER_DATA: fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-theme-')),
        QRTZ_MOCK_HELPER: '1',
        QRTZ_DEVTOOLS: '0',
        QRTZ_DEV_SEED: 'many',
      },
    });
    const win = await app.firstWindow();
    await expect(win.getByTestId('home')).toBeVisible();
    await win.getByTestId('nav-settings').click();
    await win.getByTestId('theme').selectOption(mode);
    await expect(win.locator('html')).toHaveAttribute('data-theme', mode);
    const bg = await win.evaluate(
      () => getComputedStyle(document.body).backgroundColor,
    );
    expect(bg).toBe(
      mode === 'light' ? 'rgb(255, 255, 255)' : 'rgb(10, 10, 10)',
    );

    const bad: string[] = [];
    const check = async (name: string) => {
      await win.waitForTimeout(600);
      await win.screenshot({ path: path.join(shots, `${mode}-${name}.png`) });
      bad.push(...(await axe(win, `${mode}/${name}`)));
      console.log(`${mode}/${name} ok`);
    };
    await win.getByTestId('nav-home').click();
    await expect(win.getByTestId('home').locator('h1')).toBeVisible();
    await check('home');
    await win.getByTestId('nav-memo').click();
    const item = win.getByTestId('meeting-item').first();
    await expect(item).toBeVisible({ timeout: 15_000 });
    await check('memo-list');
    await item.click();
    await expect(win.getByTestId('tab-transcript')).toBeVisible();
    await check('memo-summary');
    await win.getByTestId('tab-transcript').click();
    await win.waitForTimeout(300);
    await check('memo-transcript');
    await win.getByTestId('nav-gallery').click();
    await check('gallery');
    await win.getByTestId('nav-settings').click();
    await check('settings');
    await win.keyboard.press('ControlOrMeta+k');
    await expect(win.getByTestId('palette')).toBeVisible();
    await check('palette');
    await win.keyboard.press('Escape');
    expect(bad).toEqual([]);
    await app.close();
  });
}
