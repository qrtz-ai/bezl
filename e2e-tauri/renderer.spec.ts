import { expect, test, type Page } from '@playwright/test';
import { axe } from './axe';

// The shipped renderer + createTauriTransport against the mocked Tauri IPC backend.
async function open(page: Page, query = '') {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text());
  });
  await page.goto(`/?dev=1&${query}`);
  await expect(
    page.getByRole('button', { name: 'nano', exact: true }),
  ).toBeVisible();
  return errors;
}

const invoke = (
  page: Page,
  subApp: string,
  channel: string,
  payload?: unknown,
) =>
  page.evaluate(
    ([subApp, channel, payload]) =>
      window.qrtzTransport!.invoke({
        v: 1,
        id: `t-${Math.random()}`,
        channel: channel as string,
        version: 1,
        subApp: subApp as string,
        payload,
      }),
    [subApp, channel, payload] as const,
  );

test('boots the shared shell over the Tauri transport with no errors', async ({
  page,
}) => {
  const errors = await open(page);
  for (const tab of ['nano', 'echo', 'settings'])
    await expect(
      page.getByRole('button', { name: tab, exact: true }),
    ).toBeVisible();
  await expect(page.getByText('no meetings')).toBeVisible();
  const info = await invoke(page, 'shell', 'app:get-info');
  expect(info).toMatchObject({
    ok: true,
    data: { electron: 'tauri (mock ipc)' },
  });
  expect(errors).toEqual([]);
});

test('capability scoping holds through the Tauri transport', async ({
  page,
}) => {
  await open(page);
  expect(await invoke(page, 'gallery', 'note:list', {})).toMatchObject({
    ok: false,
    error: { code: 'E_FORBIDDEN' },
  });
  expect(await invoke(page, 'note', 'config:set', { patch: {} })).toMatchObject(
    {
      ok: false,
      error: { code: 'E_FORBIDDEN' },
    },
  );
  expect(
    await invoke(page, 'note', 'note:get', { id: 'x'.repeat(100) }),
  ).toMatchObject({
    ok: false,
    error: { code: 'E_VALIDATION' },
  });
  const calls = await page.evaluate(
    () =>
      (window as unknown as { __qrtzMock: { calls: { subApp: string }[] } })
        .__qrtzMock.calls,
  );
  expect(calls.some((c) => c.subApp === 'note')).toBe(true);
});

test('record → transcript streams word by word → stop → post-processing → done', async ({
  page,
}) => {
  const errors = await open(page, 'mockSpeed=150');
  await page.getByRole('button', { name: 'Record' }).click();
  await expect(page.getByRole('button', { name: 'Stop' })).toBeVisible();
  // A partial shows up before its final line: streamed, not batched.
  await expect(
    page.getByText("Let's start", { exact: false }).first(),
  ).toBeVisible();
  const early = await page.locator('body').innerText();
  expect(early).not.toContain('The beta ships on Friday.');
  await expect(page.getByText("Let's start with the roadmap.")).toBeVisible();
  await expect(page.getByText('The beta ships on Friday.')).toBeVisible({
    timeout: 10_000,
  });
  await page.getByRole('button', { name: 'Stop' }).click();
  await expect(page.getByText('Ready')).toBeVisible({ timeout: 10_000 });
  await expect(page.getByRole('button', { name: 'Record' })).toBeVisible();
  // Transcript stays front and center after stop; summary landed.
  await expect(page.getByText("Let's start with the roadmap.")).toBeVisible();
  await page
    .getByRole('tab', { name: 'summary' })
    .or(page.getByRole('button', { name: 'summary' }))
    .first()
    .click();
  await expect(
    page.getByText('The beta ships on Friday.').first(),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test('denied permission on start surfaces an error and leaves Record available', async ({
  page,
}) => {
  await open(page, 'mockFail=start-permission');
  await page.getByTestId('note-record-manual').click();
  await expect(page.getByTestId('lifecycle-error')).toContainText(
    'Screen & System Audio Recording or Microphone',
  );
  await expect(page.getByTestId('meeting-item')).toHaveCount(0);
  await expect(page.getByTestId('note-record-manual')).toBeVisible();
});

test('a failed summary step offers Retry and recovers', async ({ page }) => {
  await open(page, 'mockFail=summary-once&mockSpeed=40');
  await page.getByRole('button', { name: 'Record' }).click();
  await expect(page.getByText("Let's start with the roadmap.")).toBeVisible({
    timeout: 10_000,
  });
  await page.getByRole('button', { name: 'Stop' }).click();
  const retry = page.getByRole('button', { name: /retry/i }).first();
  await expect(retry).toBeVisible({ timeout: 10_000 });
  await retry.click();
  await expect(page.getByText('Ready')).toBeVisible({ timeout: 10_000 });
});

test('settings toggles write config through the backend', async ({ page }) => {
  await open(page);
  await page.getByRole('button', { name: 'settings', exact: true }).click();
  const before = (await invoke(page, 'settings', 'config:get')) as {
    data: { app: { launchAtLogin: boolean } };
  };
  const toggle = page.getByRole('switch', { name: 'Open at login' });
  await expect(toggle).toBeVisible();
  await toggle.click();
  await expect
    .poll(
      async () =>
        ((await invoke(page, 'settings', 'config:get')) as typeof before).data
          .app.launchAtLogin,
    )
    .toBe(!before.data.app.launchAtLogin);
});

test('seeded meeting opens; views pass axe', async ({ page }, info) => {
  await open(page, 'seed=1');
  await page.getByText('Weekly Product Sync').first().click();
  await expect(page.getByText("Let's start.")).toBeVisible();
  if (info.project.name === 'chromium')
    expect(await axe(page, 'tauri-note')).toEqual([]);
  await page.getByRole('button', { name: 'settings', exact: true }).click();
  if (info.project.name === 'chromium')
    expect(await axe(page, 'tauri-settings')).toEqual([]);
});
