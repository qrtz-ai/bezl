import { expect, test, type Page } from '@playwright/test';
import { axe } from './axe';

// ADR 0184: the first-run permissions onboarding (microphone, accessibility, and system audio with nano installed) before the qrtz page,
// against the mocked backend. Also the qrtz page's app rows under axe (each row's "details" button is named by
// the row's visible text, WCAG 2.5.3).

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
  ) as Promise<{
    ok: boolean;
    data?: Record<string, unknown>;
    error?: { code: string };
  }>;

const calls = (page: Page) =>
  page.evaluate(() =>
    (
      window as unknown as {
        __qrtzMock: { calls: { channel: string; subApp: string }[] };
      }
    ).__qrtzMock.calls.map((c) => c.channel),
  );

async function clean(page: Page, name: string) {
  if (test.info().project.name !== 'chromium') return;
  expect(await axe(page, name)).toEqual([]);
}

test('first launch: permissions onboarding, then the qrtz page; reopened from qrtz settings', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/?dev=1&onboarding=1&mockPerms=denied&mockSpeech=missing');
  const ob = page.getByTestId('onboarding');
  await expect(ob).toBeVisible();
  // Before the qrtz page: no sidebar, no app view.
  await expect(page.getByTestId('nav-qrtz')).toHaveCount(0);
  for (const id of ['microphone', 'screen-recording', 'accessibility'])
    await expect(page.getByTestId(`onboarding-perm-${id}-state`)).toHaveText(
      'not granted',
    );
  await expect(ob.getByRole('heading', { level: 1 })).toHaveText('set up qrtz');
  // All text lowercase, no sideways scroll.
  const text = await ob.innerText();
  expect(text).toBe(text.toLowerCase());
  expect(
    await page.evaluate(() => {
      const m = document.querySelector('[data-scroll-root]')!;
      return m.scrollWidth <= m.clientWidth;
    }),
  ).toBe(true);
  await clean(page, 'onboarding');

  // allow → the OS prompt (the mock grants), the row flips to granted and its buttons go.
  await page.getByRole('button', { name: 'allow microphone' }).click();
  await expect(page.getByTestId('onboarding-perm-microphone-state')).toHaveText(
    'granted',
  );
  await expect(
    page.getByTestId('onboarding-perm-microphone-allow'),
  ).toHaveCount(0);
  // settings opens the permission's System Settings pane.
  await page.getByTestId('onboarding-perm-accessibility-open').click();
  // Onboarding downloads nothing: models stay in each app's detail on the qrtz page.
  const seen = await calls(page);
  expect(seen).toContain('permissions:open-settings');
  for (const ch of ['speech:download', 'packages:apply'])
    expect(seen, ch).not.toContain(ch);

  // A renderer can't mark onboarding done through config: only the host writes it.
  const forged = await invoke(page, 'settings', 'config:set', {
    patch: { onboarding: { done: true } },
  });
  expect(forged.ok).toBe(false);
  expect(forged.error?.code).toBe('E_VALIDATION');

  await page.getByTestId('onboarding-continue').click();
  await expect(ob).toHaveCount(0);
  await expect(page.getByTestId('nav-qrtz')).toBeVisible();
  await expect(page.locator('[data-subapp="marketplace"]')).toBeVisible();
  const cfg = await invoke(page, 'settings', 'config:get');
  expect(cfg.data?.onboarding).toEqual({ done: true });

  // qrtz settings → permissions reopens it; skip goes back to settings.
  await page.getByTestId('qrtz-settings-open').click();
  await expect(page.getByTestId('permissions-note')).toContainText(
    'system audio',
  );
  await page.getByTestId('onboarding-reopen').click();
  await expect(page.getByTestId('onboarding')).toBeVisible();
  await expect(page.getByTestId('onboarding-perm-microphone-state')).toHaveText(
    'granted',
  );
  await page.getByTestId('onboarding-skip').click();
  await expect(page.getByTestId('onboarding')).toHaveCount(0);
  await expect(page.getByTestId('settings')).toBeVisible();
  expect(errors).toEqual([]);
});

test('without nano installed, onboarding and settings never list system audio (ADR 0184 amendment)', async ({
  page,
}) => {
  await page.goto(
    '/?dev=1&onboarding=1&mockPerms=denied&mockSpeech=missing&mockNano=missing',
  );
  const ob = page.getByTestId('onboarding');
  await expect(ob).toBeVisible();
  for (const id of ['microphone', 'accessibility'])
    await expect(page.getByTestId(`onboarding-perm-${id}-state`)).toHaveText(
      'not granted',
    );
  await expect(
    page.getByTestId('onboarding-perm-screen-recording-state'),
  ).toHaveCount(0);
  await expect(ob).not.toContainText('system audio');
  await clean(page, 'onboarding-no-nano');
  await page.getByTestId('onboarding-skip').click();
  await expect(ob).toHaveCount(0);
  await page.getByTestId('qrtz-settings-open').click();
  const note = page.getByTestId('permissions-note');
  await expect(note).toContainText('microphone and accessibility');
  await expect(note).not.toContainText('system audio');
  // Reopened: still no system audio row.
  await page.getByTestId('onboarding-reopen').click();
  await expect(
    page.getByTestId('onboarding-perm-microphone-state'),
  ).toBeVisible();
  await expect(
    page.getByTestId('onboarding-perm-screen-recording-state'),
  ).toHaveCount(0);
  const seen = await calls(page);
  expect(seen).not.toContain('permissions:request');
});

test('after onboarding qrtz opens on the qrtz page, and its app rows pass axe', async ({
  page,
}) => {
  await page.goto('/?dev=1');
  await expect(page.getByTestId('onboarding')).toHaveCount(0);
  const rows = page.locator('[data-app-row]');
  await expect(rows.first()).toBeVisible();
  // Each row's details button is named by what the row shows.
  const echo = page.getByTestId('app-open-echo');
  const name = await echo.evaluate((b) => b.getAttribute('aria-label'));
  expect(name).toBeNull();
  await expect(echo).toHaveAccessibleName(/^echo .* details$/);
  await clean(page, 'qrtz-page');
});
