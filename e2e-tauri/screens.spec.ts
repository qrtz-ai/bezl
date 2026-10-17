import { expect, test, type Page } from '@playwright/test';
import fs from 'node:fs';
import path from 'node:path';
import { axe } from './axe';

// First run, the speech setup screens and every view in both themes (e2e-parity rows 8, 12, 13, 29, 30, 39, 40),
// against the mocked backend. Screenshots go to test-results/screens, or docs/screenshots with QRTZ_DOC_SHOTS=1.
const out = process.env.QRTZ_DOC_SHOTS
  ? path.join(__dirname, '..', 'docs', 'screenshots')
  : path.join(__dirname, '..', 'test-results', 'screens');

async function shot(page: Page, name: string) {
  if (test.info().project.name !== 'chromium') return;
  fs.mkdirSync(out, { recursive: true });
  await page.evaluate(() => document.fonts.ready.then(() => undefined));
  await page.screenshot({ path: path.join(out, `${name}.png`) });
}

/** Serious/critical axe violations: none (chromium only; axe over CDP). */
async function clean(page: Page, name: string) {
  if (test.info().project.name !== 'chromium') return;
  expect(await axe(page, name)).toEqual([]);
}

function watch(page: Page) {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text());
  });
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

const tab = (page: Page, name: string) => page.getByTestId(`nav-${name}`);

test('first run → record → review walkthrough', async ({ page }) => {
  const errors = watch(page);
  await page.goto('/?dev=1&onboarding=1&mockPerms=denied&mockSpeed=60');
  await expect(page.getByTestId('onboarding-welcome')).toBeVisible();
  await expect(tab(page, 'note')).toHaveCount(0);
  await clean(page, 'onboarding-welcome');
  await shot(page, 'onboarding-welcome');
  await page.getByTestId('onboarding-start').click();

  // permissions: live state, allow (retry is the same button until granted)
  await expect(page.getByTestId('onboarding-permissions')).toBeVisible();
  const mic = page.getByTestId('perm-microphone');
  await expect(mic.getByRole('button', { name: 'allow' })).toBeVisible();
  await clean(page, 'onboarding-permissions');
  await shot(page, 'onboarding-permissions');
  await mic.getByRole('button', { name: 'allow' }).click();
  await expect(mic).toContainText('allowed');
  await page.getByTestId('onboarding-continue').click();

  // speech: size, download with progress, pause / resume, installed
  await expect(page.getByTestId('onboarding-speech')).toBeVisible();
  await expect(page.getByTestId('speech-size')).toHaveText('speech · 412 mb');
  await expect(page.getByTestId('onboarding-continue')).toHaveText('later');
  await page.getByTestId('speech-download').click();
  await expect(page.getByTestId('speech-progress')).toBeVisible();
  await page.getByTestId('speech-pause').click();
  await expect(page.getByTestId('speech-progress-text')).toContainText(
    'paused',
  );
  await clean(page, 'onboarding-speech');
  await shot(page, 'onboarding-speech');
  const paused = await page
    .getByTestId('speech-progress')
    .getAttribute('aria-valuenow');
  await page.waitForTimeout(300);
  expect(
    await page.getByTestId('speech-progress').getAttribute('aria-valuenow'),
  ).toBe(paused);
  await page.getByTestId('speech-resume').click();
  await expect(page.getByTestId('speech-installed')).toHaveText(
    'speech 1.0.0 · 412 mb',
    {
      timeout: 10_000,
    },
  );
  await page.getByTestId('onboarding-continue').click();

  // ADR 0076: no key step; speech is the last step before done, and onboarding stores no secret.
  const calls = await page.evaluate(
    () =>
      (
        window as unknown as {
          __qrtzMock: { calls: { channel: string; subApp: string }[] };
        }
      ).__qrtzMock.calls,
  );
  expect(calls.some((c) => c.channel === 'secrets:set')).toBe(false);
  // ADR 0078: opening onboarding never looks online; speech:check runs only when the user asks.
  expect(calls.some((c) => c.channel === 'speech:check')).toBe(false);

  await expect(page.getByTestId('onboarding-done')).toBeVisible();
  await page.getByTestId('onboarding-finish').click();
  await expect(tab(page, 'note')).toBeVisible();
  const cfg = (await invoke(page, 'settings', 'config:get')) as {
    data: { onboarding: { done: boolean } };
  };
  expect(cfg.data.onboarding.done).toBe(true);
  await expect(page.getByTestId('speech-notice')).toHaveCount(0);
  await shot(page, 'note-empty');

  // record → review
  await page.getByTestId('note-record-manual').click();
  await expect(page.getByText("Let's start with the roadmap.")).toBeVisible({
    timeout: 10_000,
  });
  await shot(page, 'note-live');
  await page.getByRole('button', { name: 'stop' }).first().click();
  await expect(page.getByText('ready')).toBeVisible({ timeout: 10_000 });
  await page
    .getByRole('tab', { name: 'summary' })
    .or(page.getByRole('button', { name: 'summary' }))
    .first()
    .click();
  await expect(
    page.getByText('The beta ships on Friday.').first(),
  ).toBeVisible();
  await clean(page, 'meeting-summary');
  await shot(page, 'meeting-summary');
  expect(errors).toEqual([]);
});

test('a failed speech download shows the error and retries', async ({
  page,
}) => {
  await page.goto('/?dev=1&onboarding=1&mockFail=speech-once&mockSpeed=40');
  await page.getByTestId('onboarding-start').click();
  await page.getByTestId('onboarding-continue').click();
  await page.getByTestId('speech-download').click();
  await expect(page.getByTestId('speech-error')).toContainText(
    "couldn't reach the download server",
  );
  await clean(page, 'onboarding-speech-error');
  await page.getByTestId('speech-retry').click();
  await expect(page.getByTestId('speech-installed')).toBeVisible({
    timeout: 10_000,
  });
});

test('the download continues in the background after onboarding; note shows it', async ({
  page,
}) => {
  await page.goto('/?dev=1&onboarding=1&mockSpeed=150');
  await page.getByTestId('onboarding-start').click();
  await page.getByTestId('onboarding-continue').click();
  await page.getByTestId('speech-download').click();
  await page.getByTestId('onboarding-continue').click();
  await page.getByTestId('onboarding-finish').click();
  await expect(page.getByTestId('speech-notice')).toContainText(
    'downloading speech',
  );
  await expect(page.getByTestId('speech-notice')).toHaveCount(0, {
    timeout: 10_000,
  });
});

test('note and talk point to settings while speech is not downloaded', async ({
  page,
}) => {
  await page.goto('/?dev=1&mockSpeech=missing');
  await expect(page.getByTestId('speech-notice')).toContainText(
    "speech isn't downloaded",
  );
  await tab(page, 'talk').click();
  await expect(page.getByTestId('speech-notice')).toBeVisible();
  await page.getByTestId('speech-notice-settings').click();
  await expect(page.getByTestId('speech-settings')).toBeVisible();
  await page.getByTestId('speech-download').click();
  await expect(page.getByTestId('speech-installed')).toBeVisible({
    timeout: 10_000,
  });
});

test('settings speech section: re-download, delete', async ({ page }) => {
  const errors = watch(page);
  await page.goto('/?dev=1&mockSpeed=40');
  await tab(page, 'settings').click();
  const sec = page.getByTestId('speech-settings');
  await expect(sec.getByTestId('speech-installed')).toHaveText(
    'speech 1.0.0 · 412 mb',
  );
  // ADR 0078: opening settings doesn't check for a speech update; the button does.
  const checks = () =>
    page.evaluate(
      () =>
        (
          window as unknown as {
            __qrtzMock: { calls: { channel: string }[] };
          }
        ).__qrtzMock.calls.filter((c) => c.channel === 'speech:check').length,
    );
  expect(await checks()).toBe(0);
  await sec.getByTestId('speech-check').click();
  await expect.poll(checks).toBe(1);
  await sec.getByTestId('speech-redownload').click();
  await expect(sec.getByTestId('speech-progress')).toBeVisible();
  await expect(sec.getByTestId('speech-installed')).toBeVisible({
    timeout: 10_000,
  });
  await sec.getByTestId('speech-delete').click();
  await expect(sec.getByTestId('speech-download')).toBeVisible();
  await clean(page, 'settings-speech');
  expect(errors).toEqual([]);
});

test('cold start: the note view paints within budget', async ({ page }) => {
  await page.goto('/?dev=1');
  await expect(page.getByTestId('note-app')).toBeVisible();
  const fcp = await page.evaluate(
    () =>
      performance.getEntriesByName('first-contentful-paint')[0]?.startTime ??
      performance.getEntriesByType('navigation')[0]?.duration ??
      0,
  );
  // Electron's budget was 1.5 s from launch; the vite dev server (unbundled modules) is the slow part here.
  expect(fcp).toBeGreaterThan(0);
  expect(fcp).toBeLessThan(process.env.CI ? 3000 : 1500);
});

test('palette and gallery', async ({ page }) => {
  const errors = watch(page);
  await page.goto('/?dev=1&seed=1');
  await expect(tab(page, 'note')).toBeVisible();
  const input = page.getByTestId('palette-input');
  // The shortcut listener attaches after the first render: press until the palette opens.
  await expect(async () => {
    if (!(await input.isVisible()))
      await page.keyboard.press('ControlOrMeta+k');
    await expect(input).toBeVisible({ timeout: 500 });
  }).toPass({ timeout: 10_000 });
  await input.fill('weekly');
  await expect(page.getByText('Weekly Product Sync').last()).toBeVisible();
  await clean(page, 'palette');
  await shot(page, 'palette');
  await input.fill('start recording');
  await page.keyboard.press('Enter');
  await expect(
    page.getByRole('button', { name: 'stop' }).first(),
  ).toBeVisible();
  const session = (await invoke(page, 'shell', 'note:session')) as {
    data: { phase: string };
  };
  expect(['starting', 'recording']).toContain(session.data.phase);
  await page.keyboard.press('ControlOrMeta+k');
  await input.fill('stop');
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('note-record-manual')).toBeVisible({
    timeout: 10_000,
  });
  await tab(page, 'gallery').click();
  await expect(page.getByTestId('gallery-page')).toBeVisible();
  await clean(page, 'gallery');
  await shot(page, 'gallery');
  expect(errors).toEqual([]);
});

for (const theme of ['dark', 'light'] as const) {
  test(`${theme} theme: every view renders, is screenshotted and axe-clean`, async ({
    page,
  }) => {
    const errors = watch(page);
    await page.goto(`/?dev=1&seed=1&theme=${theme}`);
    await expect(tab(page, 'note')).toBeVisible();
    // The theme applies once config:get answers.
    await expect
      .poll(() =>
        page.evaluate(() => {
          const c = getComputedStyle(document.body)
            .backgroundColor.match(/\d+/g)!
            .map(Number);
          return (c[0]! + c[1]! + c[2]!) / 3;
        }),
      )
      .toBeGreaterThan(theme === 'light' ? 128 : -1);
    if (theme === 'dark')
      expect(
        await page.evaluate(() => {
          const c = getComputedStyle(document.body)
            .backgroundColor.match(/\d+/g)!
            .map(Number);
          return (c[0]! + c[1]! + c[2]!) / 3;
        }),
      ).toBeLessThan(128);
    await page.getByText('Weekly Product Sync').first().click();
    await expect(page.getByText("Let's start.")).toBeVisible();
    await clean(page, `${theme}-note`);
    await shot(page, `${theme}-note`);
    for (const view of ['talk', 'gallery', 'settings']) {
      await tab(page, view).click();
      await expect(
        page.getByTestId(view === 'gallery' ? 'gallery-page' : view),
      ).toBeVisible();
      await clean(page, `${theme}-${view}`);
      await shot(page, `${theme}-${view}`);
    }
    expect(errors).toEqual([]);
  });
}

test('mini hud follows the light theme; notes toggle; axe', async ({
  page,
}) => {
  const errors = watch(page);
  await page.goto('/?dev=1&theme=light&mockSpeed=80#hud');
  await page.waitForFunction(() => !!window.qrtzTransport);
  await invoke(page, 'shell', 'note:start', {});
  await expect(page.getByTestId('mini-hud-clock')).toBeVisible();
  await expect(page.getByTestId('mini-hud-stop')).toBeVisible();
  const bg = await page.evaluate(() => {
    const el = document.querySelector('[data-testid="mini-hud-clock"]')!;
    let n: Element | null = el;
    while (n) {
      const c = getComputedStyle(n).backgroundColor;
      if (c && c !== 'rgba(0, 0, 0, 0)' && c !== 'transparent') return c;
      n = n.parentElement;
    }
    return getComputedStyle(document.body).backgroundColor;
  });
  const [r, g, b] = bg.match(/\d+/g)!.map(Number);
  expect((r! + g! + b!) / 3).toBeGreaterThan(128);
  await page.getByTestId('mini-hud-notes-toggle').click();
  await expect(page.getByTestId('mini-hud-note-input')).toBeVisible();
  await clean(page, 'hud-light');
  await shot(page, 'hud-light');
  expect(errors).toEqual([]);
});

test('talk: settings has a toggle for every automatic behaviour; each writes config', async ({
  page,
}) => {
  await page.goto('/?dev=1');
  await tab(page, 'settings').click();
  const keys: [string, string][] = [
    ['talk-enabled', 'enabled'],
    ['talk-warm-mic', 'warmMic'],
    ['talk-auto-cleanup', 'autoCleanup'],
    ['talk-auto-insert', 'autoInsert'],
    ['talk-restore-clipboard', 'restoreClipboard'],
    ['talk-show-pill', 'showPill'],
    ['talk-save-history', 'saveHistory'],
  ];
  const talk = async () =>
    (
      (await invoke(page, 'settings', 'config:get')) as {
        data: { talk: Record<string, boolean> };
      }
    ).data.talk;
  for (const [id, key] of keys) {
    const sw = page.getByTestId(id);
    await expect(sw).toHaveAttribute('role', 'switch');
    const before = (await talk())[key];
    await sw.click();
    await expect.poll(async () => (await talk())[key]).toBe(!before);
  }
});
