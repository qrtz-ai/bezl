import { expect, test, type Page } from '@playwright/test';
import fs from 'node:fs';
import path from 'node:path';
import { axe } from './axe';

// A fresh install (no first-run screen, ADR 0163), the speech setup in settings and every view in both themes (e2e-parity rows 8, 12, 13, 29, 30, 39, 40),
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

const mockCalls = (page: Page) =>
  page.evaluate(
    () =>
      (
        window as unknown as {
          __qrtzMock: { calls: { channel: string; subApp: string }[] };
        }
      ).__qrtzMock.calls,
  );

test('a fresh install opens straight to qrtz → settings → record → review', async ({
  page,
}) => {
  const errors = watch(page);
  // ADR 0163: no first-run screen. A fresh install (speech not downloaded, nothing granted) opens on echo, the
  // first sidebar row; nano is one click away.
  await page.goto('/?dev=1&mockSpeech=missing&mockPerms=denied&mockSpeed=60');
  await expect(page.locator('[data-subapp="talk"]')).toBeVisible();
  await tab(page, 'note').click();
  await expect(page.getByTestId('note-app')).toBeVisible();
  await expect(page.locator('[data-testid^="onboarding"]')).toHaveCount(0);
  await expect(page.getByTestId('speech-notice')).toContainText(
    "speech isn't downloaded",
  );
  await page.waitForTimeout(300);
  // Nothing downloads, asks for a permission or looks online at launch (ADR 0078 / 0160); there is no secrets
  // channel at all (ADR 0076).
  const atLaunch = (await mockCalls(page)).map((c) => c.channel);
  for (const ch of [
    'speech:download',
    'speech:check',
    'permissions:request',
    'secrets:set',
  ])
    expect(atLaunch, ch).not.toContain(ch);
  const cfg = (await invoke(page, 'settings', 'config:get')) as {
    data: Record<string, unknown>;
  };
  expect(cfg.data).not.toHaveProperty('onboarding');

  // marketplace → installed (the module manager): each module's permissions beside its switch, live state, allow.
  await tab(page, 'marketplace').click();
  const market = page.locator('[data-subapp="marketplace"]');
  await market.getByTestId('tab-installed').click();
  const mic = market.getByTestId('perm-echo-microphone');
  await expect(mic.getByRole('button', { name: 'allow' })).toBeVisible();
  await expect(market.getByTestId('perm-nano-screen-recording')).toBeVisible();
  await expect(market.getByTestId('perm-ctrl-automation')).toContainText(
    'asked on first use',
  );
  await clean(page, 'module-manager-permissions');
  await shot(page, 'module-manager-permissions');
  await mic.getByRole('button', { name: 'allow' }).click();
  await expect(mic).toContainText('allowed');
  // One grant covers every module (ADR 0154): nano's microphone row follows.
  await expect(market.getByTestId('perm-nano-microphone')).toContainText(
    'allowed',
  );

  await tab(page, 'settings').click();
  // settings → speech: size, download with progress, pause / resume, installed (the user's yes).
  const sec = page.getByTestId('speech-settings');
  await expect(sec.getByTestId('speech-size')).toHaveText('speech · 412 mb');
  await sec.getByTestId('speech-download').click();
  await expect(sec.getByTestId('speech-progress')).toBeVisible();
  await sec.getByTestId('speech-pause').click();
  await expect(sec.getByTestId('speech-progress-text')).toContainText('paused');
  await clean(page, 'settings-speech-download');
  await shot(page, 'settings-speech-download');
  const paused = await sec
    .getByTestId('speech-progress')
    .getAttribute('aria-valuenow');
  await page.waitForTimeout(300);
  expect(
    await sec.getByTestId('speech-progress').getAttribute('aria-valuenow'),
  ).toBe(paused);
  await sec.getByTestId('speech-resume').click();
  await expect(sec.getByTestId('speech-installed')).toHaveText(
    'speech 1.0.0 · 412 mb',
    { timeout: 10_000 },
  );
  expect(
    (await mockCalls(page)).some((c) => c.channel === 'speech:check'),
  ).toBe(false);

  await tab(page, 'note').click();
  await expect(page.getByTestId('speech-notice')).toHaveCount(0);
  await shot(page, 'note-empty');

  // record → review (settings stays mounted, hidden: scope to note)
  const note = page.locator('[data-subapp="note"]');
  await note.getByTestId('note-record-manual').click();
  await expect(page.getByText("Let's start with the roadmap.")).toBeVisible({
    timeout: 10_000,
  });
  await shot(page, 'note-live');
  await note.getByRole('button', { name: 'stop' }).first().click();
  await expect(note.getByText('ready', { exact: true })).toBeVisible({
    timeout: 10_000,
  });
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
  await page.goto(
    '/?dev=1&mockSpeech=missing&mockFail=speech-once&mockSpeed=40',
  );
  await tab(page, 'settings').click();
  const sec = page.getByTestId('speech-settings');
  await sec.getByTestId('speech-download').click();
  await expect(sec.getByTestId('speech-error')).toContainText(
    "couldn't reach the download server",
  );
  await clean(page, 'settings-speech-error');
  await sec.getByTestId('speech-retry').click();
  await expect(sec.getByTestId('speech-installed')).toBeVisible({
    timeout: 10_000,
  });
});

test('switching a module on is the yes to its download; it continues in the background and note shows it', async ({
  page,
}) => {
  await page.goto('/?dev=1&mockSpeech=missing&mockSpeed=150');
  await tab(page, 'marketplace').click();
  const market = page.locator('[data-subapp="marketplace"]');
  await market.getByTestId('tab-installed').click();
  const downloads = async () =>
    (await mockCalls(page)).filter((c) => c.channel === 'speech:download')
      .length;
  const sw = (n: string) => market.getByTestId(`module-enabled-${n}`);
  // Switching off never downloads; switching ctrl on doesn't either (its model has its own yes in ctrl's settings).
  await sw('echo').click();
  await expect(sw('echo')).toHaveAttribute('aria-checked', 'false');
  await sw('ctrl').click();
  await expect(sw('ctrl')).toHaveAttribute('aria-checked', 'false');
  await sw('ctrl').click();
  await expect(sw('ctrl')).toHaveAttribute('aria-checked', 'true');
  expect(await downloads()).toBe(0);
  await expect(market.getByTestId('perm-echo-microphone')).toHaveCount(0);
  // echo back on: speech starts downloading.
  await sw('echo').click();
  await expect.poll(downloads).toBe(1);
  await expect(market.getByTestId('perm-echo-microphone')).toBeVisible();
  await tab(page, 'note').click();
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
  await page.goto('/?dev=1&mockSpeech=missing#/note');
  await expect(page.getByTestId('speech-notice')).toContainText(
    "speech isn't downloaded",
  );
  await tab(page, 'talk').click();
  // Visited sub apps stay mounted (hidden), so scope to the talk app.
  const talk = page.locator('[data-subapp="talk"]');
  await expect(talk.getByTestId('speech-notice')).toBeVisible();
  await talk.getByTestId('speech-notice-open').click();
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

test('settings → open-source licenses shows the notices that ship (audit P0-4)', async ({
  page,
}) => {
  await page.goto('/?dev=1');
  await tab(page, 'settings').click();
  await page.getByTestId('licenses-open').click();
  const text = page.getByTestId('licenses-text');
  await expect(text).toContainText('Draft for legal review by Jared');
  await expect(text).toContainText(
    'Licensed by NVIDIA Corporation under the NVIDIA Open Model License',
  );
  await clean(page, 'settings-licenses');
  await page.keyboard.press('Escape');
  await expect(text).toHaveCount(0);
});

test('cold start: the note view paints within budget', async ({ page }) => {
  await page.goto('/?dev=1#/note');
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
  await page.goto('/?dev=1&seed=1#/note');
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
    await page.goto(`/?dev=1&seed=1&theme=${theme}#/note`);
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
  await page.goto('/hud.html?dev=1&theme=light&mockSpeed=80');
  await page.waitForFunction(() => !!window.qrtzTransport);
  await invoke(page, 'shell', 'note:start', {});
  await expect(page.getByTestId('mini-hud-clock')).toBeVisible();
  await expect(page.getByTestId('mini-hud-stop')).toBeVisible();
  // Its own page (ADR 0090): no shell, tabs or sub apps.
  await expect(
    page.locator('[data-subapp], [data-testid^="nav-"]'),
  ).toHaveCount(0);
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

test("talk: echo's own settings have a toggle for every automatic behaviour; each writes config", async ({
  page,
}) => {
  await page.goto('/?dev=1');
  // ADR 0146 M1.4: echo's settings live in echo's view, behind the gear.
  await tab(page, 'talk').click();
  await page.getByTestId('talk-settings-open').click();
  const keys: [string, string][] = [
    ['talk-enabled', 'enabled'],
    ['talk-warm-mic', 'warmMic'],
    ['talk-auto-cleanup', 'autoCleanup'],
    ['talk-auto-insert', 'autoInsert'],
    ['talk-restore-clipboard', 'restoreClipboard'],
    ['talk-show-pill', 'showPill'],
    ['talk-echo-gate', 'echoGate'],
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

test('no page scrolls sideways: every page fits its width, at the default and a narrow window', async ({
  page,
}) => {
  const errors = watch(page);
  const overflow = () =>
    page.evaluate(() =>
      [
        document.documentElement,
        document.body,
        document.getElementById('root'),
        document.querySelector('[data-scroll-root]'),
        ...document.querySelectorAll('[data-subapp]:not([hidden]) [data-page]'),
      ]
        .filter((e): e is HTMLElement => !!e)
        .filter((e) => e.scrollWidth > e.clientWidth)
        .map(
          (e) =>
            `${e.tagName.toLowerCase()}${e.dataset.testid ? `#${e.dataset.testid}` : ''} ${e.scrollWidth}>${e.clientWidth}`,
        ),
    );
  for (const width of [1100, 760]) {
    await page.setViewportSize({ width, height: 720 });
    await page.goto('/?dev=1&seed=1&mockSpeech=missing#/note');
    await expect(page.getByTestId('note-app')).toBeVisible();
    await page.getByText('Weekly Product Sync').first().click();
    await expect(page.getByText("Let's start.")).toBeVisible();
    expect(await overflow(), `note @${width}`).toEqual([]);
    for (const view of [
      'talk',
      'ctrl',
      'snap',
      'marketplace',
      'gallery',
      'settings',
    ]) {
      await tab(page, view).click();
      await expect(
        page.locator(`[data-subapp="${view}"] [data-page]`),
      ).toBeVisible();
      expect(await overflow(), `${view} @${width}`).toEqual([]);
    }
    const market = page.locator('[data-subapp="marketplace"]');
    await tab(page, 'marketplace').click();
    await market.getByTestId('tab-installed').click();
    expect(await overflow(), `module manager @${width}`).toEqual([]);
  }
  expect(errors).toEqual([]);
});
