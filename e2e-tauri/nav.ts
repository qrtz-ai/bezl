import { expect, type Page } from '@playwright/test';

// The sidebar has no marketplace or settings rows any more: "qrtz" (nav-qrtz) opens the qrtz page (the app
// manager, /qrtz), and qrtz's own settings sit behind that page's gear at /qrtz/settings.

/** The qrtz page's app manager (the old marketplace / module manager). */
export async function openQrtz(page: Page) {
  await page.getByTestId('nav-qrtz').click();
  // The qrtz row returns to the qrtz page's last sub-path; settings closes back to the manager.
  if (/#\/qrtz\/settings$/.test(page.url()))
    await page.getByTestId('settings-close').click();
  await expect(page).toHaveURL(/#\/qrtz$/);
  await expect(page.getByTestId('qrtz-page')).toBeVisible();
}

/** qrtz's own (global) settings: the qrtz page's gear. */
export async function openSettings(page: Page) {
  await page.getByTestId('nav-qrtz').click();
  if (!/#\/qrtz\/settings$/.test(page.url()))
    await page.getByTestId('qrtz-settings-open').click();
  await expect(page).toHaveURL(/#\/qrtz\/settings$/);
  await expect(page.getByTestId('settings')).toBeVisible();
}

/** A sidebar app, or the qrtz page's parts by their old names (`marketplace`, `settings`). */
export async function go(page: Page, name: string) {
  if (name === 'marketplace' || name === 'qrtz') return openQrtz(page);
  if (name === 'settings') return openSettings(page);
  await page.getByTestId(`nav-${name}`).click();
}
