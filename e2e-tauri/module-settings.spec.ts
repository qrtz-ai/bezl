import { expect, test, type Page } from '@playwright/test';
import { axe } from './axe';

// ADR 0146 M1.4: each module's settings live in its own view, behind a gear, and reach only that module's
// settings; the global settings keep the module switches and what spans every module.
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
type Cfg = {
  ok: boolean;
  data: { talk: { showPill: boolean }; note: { autoRecord: boolean } };
};
const config = async (page: Page) =>
  ((await invoke(page, 'settings', 'config:get')) as Cfg).data;

test('echo, nano and ctrl each open their own settings; the global settings keep only global things', async ({
  page,
}, info) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1');

  // echo
  await page.getByTestId('nav-talk').click();
  const echo = page.locator('[data-subapp="talk"]');
  await echo.getByTestId('talk-settings-open').click();
  const echoPage = echo.getByTestId('talk-settings');
  await expect(echoPage.getByText('echo settings')).toBeVisible();
  await expect(echoPage.getByTestId('auto-record')).toHaveCount(0);
  const pill = (await config(page)).talk.showPill;
  await echoPage.getByRole('switch', { name: 'floating pill' }).click();
  await expect.poll(async () => (await config(page)).talk.showPill).toBe(!pill);
  if (info.project.name === 'chromium')
    expect(await axe(page, 'tauri-echo-settings')).toEqual([]);
  await echoPage.getByTestId('talk-settings-close').click();
  await expect(echo.getByTestId('talk-settings')).toHaveCount(0);
  await expect(echo.getByTestId('talk-toggle')).toBeVisible();

  // nano: settings open in the detail pane; the meeting list stays.
  await page.getByTestId('nav-note').click();
  const nano = page.locator('[data-subapp="note"]');
  await nano.getByTestId('note-settings-open').click();
  const nanoPage = nano.getByTestId('note-settings');
  await expect(nanoPage.getByText('nano settings')).toBeVisible();
  await expect(nano.getByTestId('note-record-manual')).toBeVisible();
  await expect(nanoPage.getByTestId('talk-show-pill')).toHaveCount(0);
  const auto = (await config(page)).note.autoRecord;
  await nanoPage.getByTestId('auto-record').click();
  await expect
    .poll(async () => (await config(page)).note.autoRecord)
    .toBe(!auto);
  if (info.project.name === 'chromium')
    expect(await axe(page, 'tauri-nano-settings')).toEqual([]);

  // ctrl
  await page.getByTestId('nav-ctrl').click();
  const ctrl = page.locator('[data-subapp="ctrl"]');
  await ctrl.getByTestId('ctrl-settings-open').click();
  await expect(
    ctrl.getByTestId('ctrl-settings').getByTestId('module-settings-empty'),
  ).toBeVisible();

  // global settings
  await page.getByTestId('nav-settings').click();
  const settings = page.getByTestId('settings');
  // ADR 0161: the module switches live in the marketplace's module manager now.
  await expect(settings.getByTestId('modules-all')).toHaveCount(0);
  await expect(settings.getByTestId('open-marketplace')).toBeVisible();
  await expect(settings.getByTestId('speech-settings')).toBeVisible();
  for (const id of ['talk-show-pill', 'auto-record', 'excluded-apps'])
    await expect(settings.getByTestId(id)).toHaveCount(0);

  // A module's page can't reach another module's settings or the global ones.
  for (const [sub, patch] of [
    ['talk', { note: { autoRecord: true } }],
    ['note', { modules: { enabled: false } }],
    ['ctrl', { theme: 'light' }],
  ] as const) {
    const r = (await invoke(page, sub, 'module:set-settings', { patch })) as {
      ok: boolean;
      error?: { code: string };
    };
    expect(r.ok, sub).toBe(false);
    expect(r.error?.code).toBe('E_FORBIDDEN');
  }
  expect(errors).toEqual([]);
});
