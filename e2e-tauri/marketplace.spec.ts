import { expect, test } from '@playwright/test';
import { axe } from './axe';
import { openQrtz } from './nav';

// ADR 0161, since the qrtz page (5fce26f): the app manager lists installed and available apps; a row opens the app's
// detail modal (facts, permissions, dependencies, switch, app settings). Against the dev mock's package manager (nothing downloads).
test('qrtz page: list apps, read a detail modal, open app settings', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await openQrtz(page);
  const m = page.locator('[data-subapp="marketplace"]');
  const installed = m.getByTestId('installed-list');
  for (const n of ['echo', 'nano', 'ctrl'])
    await expect(installed.getByTestId(`app-row-${n}`)).toBeVisible();
  await expect(installed.getByTestId('app-row-nano')).toContainText(
    'with dependencies',
  );
  expect(await axe(page, 'marketplace')).toEqual([]);

  // Detail modal.
  await m.getByTestId('app-open-ctrl').click();
  const modal = page.getByTestId('app-modal');
  await expect(modal.getByTestId('fact-version')).toHaveText('v0.1.0');
  await expect(modal.getByTestId('permission-automation')).toBeVisible();
  await expect(modal.getByTestId('dep-model.laya')).toContainText('model');

  // ctrl comes with the build (config-installed, no package row), so there's nothing to remove; installing and
  // removing behind the consent is covered by the MarketplaceView vitest suite (the mock has no package to add).
  await expect(modal.getByTestId('extension-remove-ctrl')).toHaveCount(0);
  await expect(modal.getByTestId('module-enabled-ctrl')).toBeChecked();
  await page.keyboard.press('Escape');
  await expect(modal).toHaveCount(0);

  // App settings opens echo's own settings in its view.
  await m.getByTestId('app-open-echo').click();
  await page
    .getByTestId('app-modal')
    .getByTestId('module-settings-echo')
    .click();
  await expect(
    page.locator('[data-subapp="talk"]').getByTestId('talk-settings'),
  ).toBeVisible();
  expect(errors).toEqual([]);
});
