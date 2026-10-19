import { expect, test, type Page } from '@playwright/test';
import { axe } from './axe';

// ADR 0217: nano's read-only Apple Calendar against the mocked backend: nothing asks macOS until 'connect
// calendar', then the upcoming block lists the next meetings and the settings pick which calendars count.

const asks = (page: Page) =>
  page.evaluate(
    () =>
      (
        window as unknown as {
          __qrtzMock: { calls: { channel: string }[] };
        }
      ).__qrtzMock.calls.filter((c) => c.channel === 'calendar:connect').length,
  );

test('connect calendar, see upcoming, pick calendars, turn it off', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');
  await page.getByTestId('nav-note').click();
  const up = page.getByTestId('upcoming');
  await expect(page.getByTestId('upcoming-connect')).toBeVisible();
  expect(await asks(page)).toBe(0);
  await page.getByTestId('calendar-connect').click();
  await expect(page.getByTestId('upcoming-event')).toHaveCount(2);
  await expect(up.getByText('weekly sync')).toBeVisible();
  expect(await asks(page)).toBe(1);
  // No sideways scroll.
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  if (test.info().project.name === 'chromium')
    expect(await axe(page, 'tauri-nano-upcoming')).toEqual([]);

  await page.getByTestId('note-settings-open').click();
  const cal = page.getByTestId('note-settings-calendar');
  await expect(cal).toBeVisible();
  await cal.getByTestId('calendar-pick-cal-home').click();
  await page.getByTestId('note-settings-close').click();
  await expect(page.getByTestId('upcoming-event')).toHaveCount(1);
  await page.getByTestId('note-settings-open').click();
  await page.getByTestId('calendar-enabled').click();
  await page.getByTestId('note-settings-close').click();
  await expect(page.getByTestId('upcoming-event')).toHaveCount(0);
  await expect(page.getByTestId('upcoming-connect')).toBeVisible();
  expect(await asks(page)).toBe(1);
  expect(errors).toEqual([]);
});

test('onboarding lists calendar only with nano and never asks on its own', async ({
  page,
}) => {
  await page.goto('/?dev=1&onboarding=1&mockPerms=denied');
  await expect(page.getByTestId('onboarding-perm-calendar')).toBeVisible();
  expect(await asks(page)).toBe(0);
  await page.goto('/?dev=1&onboarding=1&mockPerms=denied&mockNano=missing');
  await expect(page.getByTestId('onboarding')).toBeVisible();
  await expect(page.getByTestId('onboarding-perm-calendar')).toHaveCount(0);
});
