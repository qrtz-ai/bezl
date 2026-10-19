import { expect, test } from '@playwright/test';
import { axe } from './axe';

// ADR 0161: the marketplace (browse, detail, install with consent and progress) and the module manager (switch,
// settings link, remove), against the dev mock's package manager (nothing downloads).
test('marketplace: browse, read a detail page, install with consent and progress, manage', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-marketplace').click();
  const m = page.locator('[data-subapp="marketplace"]');
  for (const n of ['echo', 'nano', 'ctrl'])
    await expect(m.getByTestId(`extension-card-${n}`)).toBeVisible();
  await expect(m.getByTestId('extension-card-echo')).toContainText(
    'with dependencies',
  );
  expect(await axe(page, 'marketplace')).toEqual([]);

  // Detail page.
  await m.getByTestId('extension-open-ctrl').click();
  await expect(m.getByTestId('fact-version')).toHaveText('v0.1.0');
  await expect(m.getByTestId('permission-automation')).toBeVisible();
  await expect(m.getByTestId('dep-model.laya')).toContainText('model');
  await m.getByTestId('marketplace-back').click();

  // ctrl ships in the build; the manager offers the signed package, behind a consent naming the download.
  await m.getByTestId('tab-installed').click();
  await m.getByTestId('module-get-ctrl').click();
  const consent = page.getByTestId('install-consent');
  await expect(consent.getByTestId('consent-download')).toContainText('491 mb');
  await consent.getByTestId('consent-confirm').click();
  await expect(m.getByTestId('module-remove-ctrl')).toBeVisible();
  await expect(m.getByTestId('module-deps-ctrl')).toContainText('model.laya');

  // The settings link opens echo's own settings in its view.
  await m.getByTestId('module-settings-echo').click();
  await expect(
    page.locator('[data-subapp="talk"]').getByTestId('talk-settings'),
  ).toBeVisible();
  expect(errors).toEqual([]);
});
