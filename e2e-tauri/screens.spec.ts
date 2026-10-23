import { expect, test, type Page } from '@playwright/test';
import fs from 'node:fs';
import path from 'node:path';
import { axe } from './axe';
import { go, openQrtz } from './nav';

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
  // Let state settle and CSS transitions finish first: a button that enables after mount fades
  // from opacity 0.55, and axe measuring mid-fade reports a false contrast failure.
  await page.evaluate(async () => {
    await new Promise((r) =>
      requestAnimationFrame(() => requestAnimationFrame(r)),
    );
    // Only transitions: spinners and other looping animations never finish.
    const fades = document
      .getAnimations()
      .filter((a) => a instanceof CSSTransition);
    await Promise.all(fades.map((a) => a.finished.catch(() => undefined)));
  });
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

const tab = (page: Page, name: string) => ({
  click: () => go(page, name),
  locator: page.getByTestId(`nav-${name}`),
});

/** The qrtz page's app manager and an app's detail modal there. */
const appModal = (page: Page) => {
  const market = page.locator('[data-subapp="marketplace"]');
  const modal = page.getByTestId('app-modal');
  return {
    market,
    modal,
    open: async (name: string) => {
      await go(page, 'marketplace');
      await market.getByTestId(`app-open-${name}`).click();
      await expect(modal).toBeVisible();
    },
    close: async () => {
      await page.keyboard.press('Escape');
      await expect(modal).toHaveCount(0);
    },
  };
};

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
  // ADR 0163: no first-run screen. A fresh install (speech not downloaded, nothing granted) opens on the top
  // sidebar app (echo, QRT-269); the apps page and nano are one click away.
  await page.goto('/?dev=1&mockSpeech=missing&mockPerms=denied&mockSpeed=150');
  await expect(page).toHaveURL(/#\/talk$/);
  await expect(page.locator('[data-subapp="talk"]')).toBeVisible();
  await openQrtz(page);
  await tab(page, 'note').click();
  await expect(page.getByTestId('note-app')).toBeVisible();
  await expect(page.locator('[data-testid^="onboarding"]')).toHaveCount(0);
  await expect(page.getByTestId('speech-notice')).toContainText(
    'not downloaded',
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
  // ADR 0184: the mock starts past the permissions onboarding (onboarding.spec.ts covers the first run).
  expect(cfg.data.onboarding).toEqual({ done: true });

  // The qrtz page: each app's permissions beside its switch in its detail modal, live state, allow.
  await tab(page, 'marketplace').click();
  const market = page.locator('[data-subapp="marketplace"]');
  const modal = page.getByTestId('app-modal');
  const openApp = async (name: string) => {
    await market.getByTestId(`app-open-${name}`).click();
    await expect(modal).toBeVisible();
  };
  const closeApp = async () => {
    await page.keyboard.press('Escape');
    await expect(modal).toHaveCount(0);
  };
  await openApp('echo');
  const mic = modal.getByTestId('perm-echo-microphone');
  await expect(mic.getByRole('button', { name: 'allow' })).toBeVisible();
  await clean(page, 'module-manager-permissions');
  await shot(page, 'module-manager-permissions');
  await mic.getByRole('button', { name: 'allow' }).click();
  await expect(mic).toContainText('allowed');
  await closeApp();
  await openApp('ctrl');
  // ctrl needs accessibility only (no automation since the permissions review).
  await expect(modal.getByTestId('perm-ctrl-accessibility')).toBeVisible();
  await expect(modal.getByTestId('perm-ctrl-automation')).toHaveCount(0);
  await closeApp();
  await openApp('nano');
  await expect(modal.getByTestId('perm-nano-screen-recording')).toBeVisible();
  // One grant covers every module (ADR 0154): nano's microphone row follows.
  await expect(modal.getByTestId('perm-nano-microphone')).toContainText(
    'allowed',
  );

  // nano's models (they moved here from settings → speech): size, download with progress, pause / resume,
  // installed (the user's yes).
  const sec = modal.getByTestId('models-nano-panel');
  await expect(sec.getByTestId('models-nano-panel-state')).toContainText(
    'not downloaded',
  );
  await sec.getByTestId('models-nano-panel-download').click();
  await expect(sec.getByTestId('models-nano-panel-progress')).toBeVisible();
  await sec.getByTestId('models-nano-panel-pause').click();
  await expect(sec).toContainText('paused');
  await clean(page, 'settings-speech-download');
  await shot(page, 'settings-speech-download');
  const paused = await sec
    .getByTestId('models-nano-panel-progress')
    .getAttribute('aria-valuenow');
  await page.waitForTimeout(300);
  expect(
    await sec
      .getByTestId('models-nano-panel-progress')
      .getAttribute('aria-valuenow'),
  ).toBe(paused);
  await sec.getByTestId('models-nano-panel-resume').click();
  await expect(sec.getByTestId('models-nano-panel-state')).toContainText(
    'on this mac',
    { timeout: 10_000 },
  );
  expect(
    (await mockCalls(page)).some((c) => c.channel === 'speech:check'),
  ).toBe(false);
  await closeApp();

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
  // The models live in each app's detail modal on the qrtz page (they moved there from settings → speech).
  const app = appModal(page);
  await app.open('nano');
  const sec = app.modal.getByTestId('models-nano-panel');
  await sec.getByTestId('models-nano-panel-download').click();
  await expect(
    sec.getByTestId('models-nano-panel-download-error'),
  ).toContainText("couldn't reach the download server");
  await clean(page, 'settings-speech-error');
  await sec.getByTestId('models-nano-panel-retry').click();
  await expect(sec.getByTestId('models-nano-panel-state')).toContainText(
    'on this mac',
    { timeout: 10_000 },
  );
});

test('switching an app off and on downloads nothing (ADR 0169); its permissions follow the switch', async ({
  page,
}) => {
  await page.goto('/?dev=1&mockSpeech=missing&mockSpeed=150');
  const app = appModal(page);
  const downloads = async () =>
    (await mockCalls(page)).filter((c) => c.channel === 'speech:download')
      .length;
  await app.open('echo');
  const sw = app.modal.getByTestId('module-enabled-echo');
  await expect(app.modal.getByTestId('perm-echo-microphone')).toBeVisible();
  await sw.click();
  await expect(sw).toHaveAttribute('aria-checked', 'false');
  await expect(app.modal.getByTestId('perm-echo-microphone')).toHaveCount(0);
  await sw.click();
  await expect(sw).toHaveAttribute('aria-checked', 'true');
  await expect(app.modal.getByTestId('perm-echo-microphone')).toBeVisible();
  await app.close();
  await app.open('ctrl');
  const ctrl = app.modal.getByTestId('module-enabled-ctrl');
  await ctrl.click();
  await expect(ctrl).toHaveAttribute('aria-checked', 'false');
  await ctrl.click();
  await expect(ctrl).toHaveAttribute('aria-checked', 'true');
  await page.waitForTimeout(300);
  expect(await downloads()).toBe(0);
});

test("note points to nano's models and echo offers its own download in place while speech is not downloaded", async ({
  page,
}) => {
  await page.goto('/?dev=1&mockSpeech=missing#/note');
  const notice = page.getByTestId('speech-notice');
  await expect(notice).toContainText('speech model');
  await expect(notice).toContainText('not downloaded');
  // The notice opens nano's detail modal on the qrtz page, at its models (ADR 0175).
  await page.getByTestId('speech-notice-open').click();
  await expect(page).toHaveURL(/#\/qrtz\/item\/nano$/);
  await expect(
    page.getByTestId('app-modal').getByTestId('models-nano-panel'),
  ).toBeVisible();
  await page.keyboard.press('Escape');
  // echo (on by default) offers parakeet in place, with the consent beside it.
  await tab(page, 'talk').click();
  const talk = page.locator('[data-subapp="talk"]');
  await expect(talk.getByTestId('echo-offer')).toBeVisible();
  await talk.getByTestId('echo-download').click();
  await expect(talk.getByTestId('echo-offer')).toHaveCount(0, {
    timeout: 10_000,
  });
});

test("an app's models: installed, delete behind a confirm, download again", async ({
  page,
}) => {
  const errors = watch(page);
  await page.goto('/?dev=1&mockSpeed=40');
  const app = appModal(page);
  await app.open('nano');
  const sec = app.modal.getByTestId('models-nano-panel');
  await expect(sec.getByTestId('models-nano-panel-state')).toContainText(
    'on this mac',
  );
  // ADR 0078: opening the models doesn't check for a speech update.
  expect(
    (await mockCalls(page)).filter((c) => c.channel === 'speech:check').length,
  ).toBe(0);
  await sec.getByTestId('models-nano-panel-delete').click();
  await page.getByTestId('models-nano-confirm-delete').click();
  await expect(sec.getByTestId('models-nano-panel-download')).toBeVisible();
  await sec.getByTestId('models-nano-panel-download').click();
  await expect(sec.getByTestId('models-nano-panel-state')).toContainText(
    'on this mac',
    { timeout: 10_000 },
  );
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

test('palette', async ({ page }) => {
  const errors = watch(page);
  await page.goto('/?dev=1&seed=1#/note');
  await expect(tab(page, 'note').locator).toBeVisible();
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
  expect(errors).toEqual([]);
});

for (const theme of ['dark', 'light'] as const) {
  test(`${theme} theme: every view renders, is screenshotted and axe-clean`, async ({
    page,
  }) => {
    const errors = watch(page);
    await page.goto(`/?dev=1&seed=1&theme=${theme}#/note`);
    await expect(tab(page, 'note').locator).toBeVisible();
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
    for (const view of ['talk', 'settings']) {
      await tab(page, view).click();
      await expect(page.getByTestId(view)).toBeVisible();
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
  // ADR 0146 M1.4: echo's settings are echo's own, behind its sidebar row's ⋯.
  await tab(page, 'talk').click();
  await page.getByTestId('nav-talk-opts').click();
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
    await page.goto('/?dev=1&seed=1#/note');
    await expect(page.getByTestId('note-app')).toBeVisible();
    await page.getByText('Weekly Product Sync').first().click();
    await expect(page.getByText("Let's start.")).toBeVisible();
    expect(await overflow(), `note @${width}`).toEqual([]);
    for (const view of ['talk', 'ctrl', 'snap', 'marketplace', 'settings']) {
      await tab(page, view).click();
      await expect(
        page.locator(`[data-subapp="${view}"] [data-page]`),
      ).toBeVisible();
      expect(await overflow(), `${view} @${width}`).toEqual([]);
    }
    const market = page.locator('[data-subapp="marketplace"]');
    await tab(page, 'marketplace').click();
    await market.getByTestId('app-open-nano').click();
    await expect(page.getByTestId('app-modal')).toBeVisible();
    expect(await overflow(), `app detail @${width}`).toEqual([]);
    await page.keyboard.press('Escape');
  }
  expect(errors).toEqual([]);
});
