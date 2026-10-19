import { expect, type Page } from '@playwright/test';

// The sidebar's bottom rows are the qrtz page's two halves: "apps" (nav-apps) opens the app manager at /qrtz,
// and "opts" (nav-opts) opens qrtz's own opts at /qrtz/settings.

/** The qrtz page's app manager (the old marketplace / module manager). */
export async function openQrtz(page: Page) {
  await page.getByTestId('nav-apps').click();
  await expect(page).toHaveURL(/#\/qrtz$/);
  await expect(page.getByTestId('qrtz-page')).toBeVisible();
}

/** qrtz's own (global) opts. */
export async function openSettings(page: Page) {
  await page.getByTestId('nav-opts').click();
  await expect(page).toHaveURL(/#\/qrtz\/settings$/);
  await expect(page.getByTestId('settings')).toBeVisible();
}

/** A sidebar app, or the qrtz page's parts by their old names (`marketplace`, `settings`). */
export async function go(page: Page, name: string) {
  if (name === 'marketplace' || name === 'qrtz' || name === 'apps')
    return openQrtz(page);
  if (name === 'settings' || name === 'opts') return openSettings(page);
  // The dev-only gallery has no sidebar row; it is only reachable by its route.
  if (name === 'gallery') {
    await page.evaluate(() => (location.hash = '#/gallery'));
    return;
  }
  await page.getByTestId(`nav-${name}`).click();
}
