// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import {
  defaultConfig,
  onNavigateRequest,
  type NavRequest,
  type QrtzClient,
} from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { ipcSamples } from '@qrtz/core/ipc/samples';
import { primaryButtons, ThemeProvider, ToastProvider } from '@qrtz/ui';
import { AppError } from '@qrtz/core';
import MarketplaceView from './MarketplaceView';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

type Call = { channel: string; payload: unknown };

/** A fake client over the golden samples that records calls and lets the test push events. */
function fake(overrides: Record<string, (p: unknown) => unknown> = {}) {
  const calls: Call[] = [];
  const listeners = new Map<string, Set<(p: unknown) => void>>();
  let config = defaultConfig();
  const client = {
    subApp: 'marketplace',
    invoke: async (channel: string, payload?: unknown) => {
      calls.push({ channel, payload });
      if (overrides[channel]) return overrides[channel](payload);
      if (channel === 'config:get') return config;
      if (channel === 'packages:set-enabled') {
        const { name, enabled } = payload as { name: 'ctrl'; enabled: boolean };
        config = {
          ...config,
          modules: {
            ...config.modules,
            [name]: { ...config.modules[name], enabled },
          },
        };
        return { name, enabled };
      }
      return (ipcSamples as Record<string, { response: unknown }>)[channel]
        ?.response;
    },
    on: (event: string, fn: (p: unknown) => void) => {
      const s = listeners.get(event) ?? new Set();
      s.add(fn);
      listeners.set(event, s);
      return () => s.delete(fn);
    },
  } as unknown as QrtzClient;
  const push = (event: string, p: unknown) =>
    listeners.get(event)?.forEach((f) => f(p));
  return { client, calls, push };
}

let root: Root | null = null;
let el: HTMLElement;
afterEach(() => {
  act(() => root?.unmount());
  root = null;
  el?.remove();
});

const settle = () =>
  act(async () => {
    await new Promise((r) => setTimeout(r, 10));
  });

async function render(client: QrtzClient) {
  el = document.createElement('div');
  document.body.appendChild(el);
  root = createRoot(el);
  await act(async () =>
    root!.render(
      <ThemeProvider mode="light">
        <ToastProvider>
          <QrtzClientProvider client={client}>
            <MarketplaceView />
          </QrtzClientProvider>
        </ToastProvider>
      </ThemeProvider>,
    ),
  );
  await settle();
}
const q = (id: string) =>
  document.body.querySelector<HTMLElement>(`[data-testid="${id}"]`);
const click = async (id: string) => {
  const b = q(id);
  expect(b, id).not.toBeNull();
  await act(async () => b!.click());
  await settle();
};
/** Click a row itself (not its action): the row's clickable surface. */
const clickRow = async (name: string) => {
  const r = q(`app-row-${name}`)?.firstElementChild as HTMLElement | null;
  expect(r, name).not.toBeNull();
  await act(async () => r!.click());
  await settle();
};
const key = async (
  k: string,
  target: EventTarget = document.activeElement ?? document.body,
) => {
  await act(async () => {
    target.dispatchEvent(
      new KeyboardEvent('keydown', { key: k, bubbles: true }),
    );
  });
  await settle();
};
/** The labels of an app's "more actions" menu, opened. */
const menuItems = async (name: string) => {
  await click(`app-menu-${name}`);
  const items = [
    ...(q(`app-menu-${name}-menu`)?.querySelectorAll('[role="menuitem"]') ??
      []),
  ].map((b) => b.getAttribute('data-testid')!.replace(`app-menu-${name}-`, ''));
  await key('Escape', q(`app-menu-${name}-menu`)!);
  return items;
};
const list = ipcSamples['packages:list'].response;
/** A non-bundled app installed from the index (removable, unlike bundled echo). */
const weatherRow = {
  ...list.packages[0]!,
  name: 'weather',
  version: '0.1.0',
};
const weatherEntry = {
  ...list.available[0]!,
  name: 'weather',
  version: '0.1.0',
  versions: ['0.1.0'],
  permissions: [],
};

describe('the qrtz page (ADR 0161, Jared 2026-10-18)', () => {
  it('puts the signed list line and "check for apps" at the top of available apps', async () => {
    const f = fake();
    await render(f.client);
    const available = q('qrtz-available')!;
    expect(available.contains(q('marketplace-index'))).toBe(true);
    expect(available.contains(q('marketplace-check'))).toBe(true);
    expect(q('qrtz-installed')!.contains(q('marketplace-index'))).toBe(false);
  });

  it('lists installed apps in short rows, without going online', async () => {
    const f = fake();
    await render(f.client);
    expect(primaryButtons(document.body)).toBeLessThanOrEqual(1);
    const page = q('qrtz-page')!;
    // Pages have no title row: the sidebar's "apps" row names it.
    expect(page.querySelector('h1')).toBeNull();
    // opts is its own sidebar row now: no gear on the apps page.
    expect(q('qrtz-settings-open')).toBeNull();
    for (const n of ['echo', 'nano', 'ctrl'])
      expect(
        q('installed-list')?.querySelector(`[data-testid="app-row-${n}"]`),
        n,
      ).not.toBeNull();
    // Every index app is installed here: the available list says so.
    expect(q('available-none')?.textContent).toContain(
      'every app is installed',
    );
    // echo: its lock row's version, and its size with what it pulls in.
    expect(q('app-meta-echo')?.textContent).toMatch(/^v0\.3\.0 · \d+ mb$/);
    // Rows, not cards.
    expect(document.querySelector('[data-card-grid]')).toBeNull();
    expect(document.querySelector('[data-grid-card]')).toBeNull();
    expect(f.calls.map((c) => c.channel)).not.toContain('packages:refresh');
    expect(el.textContent).toBe(el.textContent?.toLowerCase());
  });

  it('checks for apps only when asked', async () => {
    const f = fake();
    await render(f.client);
    await click('marketplace-check');
    expect(
      f.calls.filter((c) => c.channel === 'packages:refresh'),
    ).toHaveLength(1);
  });

  it("says 'coming soon' instead of a raw error while modules.qrtz.ai isn't live", async () => {
    const f = fake({
      'packages:refresh': () => {
        throw new AppError(
          'E_UNAVAILABLE',
          "apps coming soon. apps will appear here once qrtz.ai's app server is live.",
          { retryable: false, details: { reason: 'not-set-up' } },
        );
      },
    });
    await render(f.client);
    await click('marketplace-check');
    expect(q('marketplace-error')).toBeNull();
    const soon = q('marketplace-soon');
    expect(soon?.textContent).toContain('apps coming soon');
    expect(soon?.textContent).toContain(
      "apps will appear here once qrtz.ai's app server is live",
    );
    expect(soon?.textContent).not.toContain('root');
    // The installed apps still list.
    expect(q('app-row-echo')).not.toBeNull();
  });
});

describe('available apps: a download icon, then an inline progress bar', () => {
  const empty = { ...list, packages: [] };

  it('downloads through a plan the user confirms; the row shows one progress bar for the package and its models', async () => {
    let release = () => {};
    const g = fake({
      'packages:list': () => ({ ...empty, available: [weatherEntry] }),
      'packages:apply': () =>
        new Promise((r) => {
          release = () => r(ipcSamples['packages:apply'].response);
        }),
    });
    await render(g.client);
    expect(
      q('available-list')?.querySelector('[data-testid="app-row-weather"]'),
    ).not.toBeNull();
    const dl = q('app-download-weather')!;
    expect(dl.getAttribute('aria-label')).toBe('download weather');
    expect(dl.textContent).toBe('');
    await click('app-download-weather');
    // Clicking the icon never opens the row's modal.
    expect(q('app-modal')).toBeNull();
    expect(g.calls.find((c) => c.channel === 'packages:plan')?.payload).toEqual(
      { op: 'install', names: ['weather'] },
    );
    // Nothing applies until the consent is confirmed; the consent names the download (package and models).
    expect(g.calls.map((c) => c.channel)).not.toContain('packages:apply');
    expect(q('consent-download')?.textContent).toContain('629 mb');
    expect(q('consent-changes')?.textContent).toContain(
      'add model.parakeet-tdt-v2-en',
    );
    await click('consent-confirm');
    expect(
      g.calls.find((c) => c.channel === 'packages:apply')?.payload,
    ).toEqual({
      planSha256: ipcSamples['packages:plan'].response.planSha256,
      downloadBytes: ipcSamples['packages:plan'].response.downloadBytes,
    });
    // The icon is gone; a progress bar is in its place.
    expect(q('app-download-weather')).toBeNull();
    await act(async () =>
      g.push('packages:progress', {
        doneBytes: 314_000_000,
        totalBytes: 628_500_000,
      }),
    );
    const bar = q('app-progress-weather')!;
    expect(bar.getAttribute('role')).toBe('progressbar');
    expect(bar.getAttribute('aria-valuenow')).toBe('50');
    expect(bar.getAttribute('aria-label')).toBe('installing weather · 50%');
    expect(q('app-progress-weather-wrap')?.getAttribute('title')).toBe('50%');
    await act(async () => release());
    await settle();
    expect(q('app-progress-weather')).toBeNull();
    // The list is read again after the install.
    expect(
      g.calls.filter((c) => c.channel === 'packages:list').length,
    ).toBeGreaterThan(1);
  });

  it('a failed download goes back to an icon, with an error and a retry', async () => {
    let tries = 0;
    const g = fake({
      'packages:list': () => ({ ...empty, available: [weatherEntry] }),
      'packages:apply': () => {
        tries++;
        throw new AppError('E_INTERNAL', 'checksum mismatch', {
          retryable: true,
        });
      },
    });
    await render(g.client);
    await click('app-download-weather');
    await click('consent-confirm');
    expect(tries).toBe(1);
    expect(q('app-progress-weather')).toBeNull();
    expect(q('app-download-weather')).toBeNull();
    const retry = q('app-retry-weather')!;
    expect(retry.getAttribute('aria-label')).toBe('retry downloading weather');
    expect(q('app-status-weather')?.textContent).toBe("couldn't download");
    expect(q('marketplace-error')?.textContent).toContain('checksum mismatch');
    await click('app-retry-weather');
    expect(g.calls.filter((c) => c.channel === 'packages:plan')).toHaveLength(
      2,
    );
    expect(q('install-consent')).not.toBeNull();
  });

  it('an index app is available, not built in, until installed', async () => {
    const cfg = defaultConfig();
    for (const id of ['echo', 'nano', 'ctrl', 'snap'] as const)
      cfg.modules[id].installed = false;
    const f = fake({
      'config:get': () => cfg,
      'packages:list': () => ({ ...list, packages: [] }),
    });
    await render(f.client);
    expect(
      q('available-list')?.querySelector('[data-testid="app-row-echo"]'),
    ).not.toBeNull();
    expect(q('app-download-echo')).not.toBeNull();
    expect(q('app-menu-echo')).toBeNull();
    expect(q('app-status-echo')).toBeNull();
  });
});

describe("an app's detail modal", () => {
  it('opens on a row click with the full detail, and closes with its button, esc or the scrim', async () => {
    await render(fake().client);
    expect(q('app-modal')).toBeNull();
    await clickRow('echo');
    const modal = q('app-modal')!;
    expect(modal.getAttribute('role')).toBe('dialog');
    expect(modal.getAttribute('aria-modal')).toBe('true');
    expect(q('fact-version')?.textContent).toBe('v0.3.0');
    expect(q('fact-install-size')?.textContent).toContain('629 mb');
    expect(q('permission-microphone')).not.toBeNull();
    expect(q('permission-accessibility')).not.toBeNull();
    expect(q('dep-inference.voice')?.textContent).toContain('service');
    expect(q('dep-model.parakeet-tdt-v2-en')?.textContent).toContain(
      'via a dependency',
    );
    expect(q('changelog-0.3.0')?.textContent).toContain('installed');
    expect(q('changelog-0.2.0')).not.toBeNull();
    expect(q('extension-launch-echo')).not.toBeNull();
    // Focus moved into it.
    expect(modal.contains(document.activeElement)).toBe(true);
    await click('app-modal-close');
    expect(q('app-modal')).toBeNull();

    await click('app-open-nano');
    expect(q('app-modal')).not.toBeNull();
    await key('Escape');
    expect(q('app-modal')).toBeNull();

    await clickRow('ctrl');
    const scrim = q('app-modal-scrim')!;
    await act(async () => {
      scrim.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    });
    await settle();
    expect(q('app-modal')).toBeNull();
  });

  it('keeps tab inside the modal (focus trap)', async () => {
    await render(fake().client);
    await clickRow('echo');
    const modal = q('app-modal')!;
    const focusables = [
      ...modal.querySelectorAll<HTMLElement>('button:not([disabled])'),
    ];
    focusables[focusables.length - 1]!.focus();
    await key('Tab');
    expect(document.activeElement).toBe(focusables[0]);
    await act(async () => {
      document.activeElement!.dispatchEvent(
        new KeyboardEvent('keydown', {
          key: 'Tab',
          shiftKey: true,
          bubbles: true,
        }),
      );
    });
    expect(document.activeElement).toBe(focusables[focusables.length - 1]);
  });

  it('esc closes only the consent over the modal, then the modal', async () => {
    const f = fake({
      'packages:list': () => ({
        ...list,
        packages: [...list.packages, weatherRow],
        available: [...list.available, weatherEntry],
      }),
    });
    await render(f.client);
    await clickRow('weather');
    await click('extension-remove-weather');
    expect(q('install-consent')).not.toBeNull();
    await key('Escape');
    expect(q('install-consent')).toBeNull();
    expect(q('app-modal')).not.toBeNull();
    await key('Escape');
    expect(q('app-modal')).toBeNull();
  });

  it('echo has no remove; its app settings are in the modal', async () => {
    await render(fake().client);
    await clickRow('echo');
    expect(q('extension-remove-echo')).toBeNull();
    expect(q('module-settings-echo')?.textContent).toBe('app opts');
  });

  it('switches an app off through packages:set-enabled only', async () => {
    const f = fake();
    await render(f.client);
    await clickRow('ctrl');
    const sw = q('module-enabled-ctrl')!;
    expect(sw.getAttribute('aria-checked')).toBe('true');
    await act(async () => sw.click());
    await settle();
    expect(
      f.calls.find((c) => c.channel === 'packages:set-enabled')?.payload,
    ).toEqual({ name: 'ctrl', enabled: false });
    expect(f.calls.map((c) => c.channel)).not.toContain('config:set');
    expect(q('module-enabled-ctrl')?.getAttribute('aria-checked')).toBe(
      'false',
    );
    // Its settings can't open while it is off, and its row says so.
    expect(
      (q('module-settings-ctrl') as HTMLButtonElement | null)?.disabled,
    ).toBe(true);
    expect(q('app-status-ctrl')?.textContent).toBe('off');
  });

  it('removes a non-bundled app through a plan, then offers clean up', async () => {
    let removed = false;
    const f = fake({
      'packages:list': () =>
        removed
          ? {
              ...list,
              reclaimable: { dirs: 3, blobs: 4, bytes: 628_500_000 },
            }
          : { ...list, packages: [...list.packages, weatherRow] },
      'packages:apply': () => {
        removed = true;
        return ipcSamples['packages:apply'].response;
      },
    });
    await render(f.client);
    expect(q('module-prune')).toBeNull();
    await click('app-menu-weather');
    await click('app-menu-weather-remove');
    expect(q('app-modal')).toBeNull();
    expect(f.calls.find((c) => c.channel === 'packages:plan')?.payload).toEqual(
      { op: 'remove', names: ['weather'] },
    );
    expect(q('install-consent')).not.toBeNull();
    await click('consent-confirm');
    expect(f.calls.map((c) => c.channel)).toContain('packages:apply');
    expect(q('module-prune')?.textContent).toContain('629 mb');
    await click('module-prune');
    expect(f.calls.map((c) => c.channel)).toContain('packages:prune');
  });
});

describe('installed apps: a three-dots menu', () => {
  it('lists open and app settings; remove only for a non-bundled app (echo has none)', async () => {
    await render(
      fake({
        'packages:list': () => ({
          ...list,
          packages: [...list.packages, weatherRow],
          available: [...list.available, weatherEntry],
        }),
      }).client,
    );
    const b = q('app-menu-echo')!;
    expect(b.getAttribute('aria-label')).toBe('more actions for echo');
    expect(b.getAttribute('aria-haspopup')).toBe('menu');
    expect(b.hasAttribute('data-badge')).toBe(false);
    expect(await menuItems('echo')).toEqual(['open', 'settings']);
    // weather has no view in this build: no open or settings, but it can be removed (a danger item).
    expect(await menuItems('weather')).toEqual(['remove']);
    await click('app-menu-weather');
    expect(q('app-menu-weather-remove')?.hasAttribute('data-danger')).toBe(
      true,
    );
  });

  it('shows update (with a dot on the button) when the index has a newer version', async () => {
    const old = { ...list.packages[0]!, version: '0.2.0' };
    const f = fake({
      'packages:list': () => ({
        ...list,
        packages: [old, ...list.packages.slice(1)],
      }),
    });
    await render(f.client);
    expect(q('app-menu-echo')?.hasAttribute('data-badge')).toBe(true);
    expect(q('app-menu-echo-badge')).not.toBeNull();
    expect(q('app-status-echo')?.textContent).toBe('update');
    expect(await menuItems('echo')).toEqual(['open', 'update', 'settings']);
    await click('app-menu-echo');
    await click('app-menu-echo-update');
    expect(f.calls.find((c) => c.channel === 'packages:plan')?.payload).toEqual(
      { op: 'update', names: ['echo'], latest: true },
    );
  });

  it('shows restart to update when an update waits for a restart (ADR 0172)', async () => {
    const f = fake({
      'packages:list': () => ({ ...list, pendingRestart: ['echo'] }),
    });
    await render(f.client);
    expect(q('app-status-echo')?.textContent).toBe('restart to update');
    expect(await menuItems('echo')).toEqual(['open', 'restart', 'settings']);
    await click('app-menu-echo');
    await click('app-menu-echo-restart');
    expect(f.calls.map((c) => c.channel)).toContain('app:restart');
  });

  it('app settings opens the app view at its settings page; clicks in the menu never open the modal', async () => {
    await render(fake().client);
    const seen: NavRequest[] = [];
    const off = onNavigateRequest((r) => seen.push(r));
    await click('app-menu-echo');
    expect(q('app-modal')).toBeNull();
    await click('app-menu-echo-settings');
    await click('app-menu-echo');
    await click('app-menu-echo-open');
    off();
    expect(seen).toEqual([{ app: 'talk', settings: true }, { app: 'talk' }]);
    expect(q('app-modal')).toBeNull();
  });

  it('works from the keyboard: arrows move, escape closes and returns focus', async () => {
    await render(fake().client);
    const b = q('app-menu-echo')!;
    b.focus();
    await key('ArrowDown', b);
    const menu = q('app-menu-echo-menu')!;
    expect(menu.getAttribute('role')).toBe('menu');
    expect(b.getAttribute('aria-expanded')).toBe('true');
    expect(document.activeElement).toBe(q('app-menu-echo-open'));
    await key('ArrowDown');
    expect(document.activeElement).toBe(q('app-menu-echo-settings'));
    await key('ArrowDown');
    expect(document.activeElement).toBe(q('app-menu-echo-open'));
    await key('End');
    expect(document.activeElement).toBe(q('app-menu-echo-settings'));
    await key('Escape');
    expect(q('app-menu-echo-menu')).toBeNull();
    expect(document.activeElement).toBe(b);
    // Keys in the menu never reached the row or opened a modal.
    expect(q('app-modal')).toBeNull();
  });
});

describe("each app's models, in its modal (moved from settings → speech)", () => {
  const missing = {
    ...ipcSamples['speech:status'].response,
    installed: null,
    bytes: 0,
    download: { state: 'idle', done: 0, total: 0 },
  };
  const speechCalls = (f: ReturnType<typeof fake>) =>
    f.calls.filter((c) => c.channel.startsWith('speech:'));

  it("lists the app's model sets and their size on disk; delete asks first and is scoped to the app", async () => {
    const f = fake();
    await render(f.client);
    await clickRow('nano');
    const panel = q('models-nano-panel')!;
    for (const set of ['nemotron-560ms', 'nemotron3-diarizer', 'localvqe'])
      expect(q(`models-nano-panel-set-${set}`), set).not.toBeNull();
    expect(q('models-nano-panel-state')?.textContent).toBe(
      'on this mac · 412 mb',
    );
    expect(panel.textContent).not.toContain('kept');
    await click('models-nano-panel-delete');
    // Nothing is deleted until confirmed.
    expect(f.calls.map((c) => c.channel)).not.toContain('speech:delete');
    expect(q('models-nano-confirm')).not.toBeNull();
    await click('models-nano-confirm-delete');
    expect(f.calls.find((c) => c.channel === 'speech:delete')?.payload).toEqual(
      { extensions: ['nano'] },
    );
    // Every speech call names exactly this app: there is no unscoped "delete everything".
    for (const c of speechCalls(f))
      expect(c.payload, c.channel).toEqual({ extensions: ['nano'] });
  });

  it('offers download when the models are missing, and retry when it failed', async () => {
    let st: unknown = missing;
    const f = fake({
      'speech:status': () => st,
      'speech:download': () => {
        st = {
          ...missing,
          download: {
            state: 'error',
            done: 0,
            total: 0,
            error: "couldn't reach huggingface.co",
          },
        };
        return st;
      },
    });
    await render(f.client);
    await clickRow('echo');
    expect(q('models-echo-panel-state')?.textContent).toBe(
      'not downloaded · 412 mb',
    );
    await click('models-echo-panel-download');
    expect(
      f.calls.find((c) => c.channel === 'speech:download')?.payload,
    ).toEqual({ extensions: ['echo'] });
    expect(q('models-echo-panel-download-error')?.textContent).toContain(
      "couldn't reach huggingface.co",
    );
    await click('models-echo-panel-retry');
    expect(f.calls.filter((c) => c.channel === 'speech:download')).toHaveLength(
      2,
    );
    for (const c of speechCalls(f))
      expect(c.payload, c.channel).toEqual({ extensions: ['echo'] });
  });

  it("says plainly when the host keeps qrtz's one shared speech bundle", async () => {
    const f = fake({
      'speech:delete': () => {
        throw new AppError(
          'E_VALIDATION',
          'the speech module is shared: remove it in opts',
          { retryable: false },
        );
      },
    });
    await render(f.client);
    await clickRow('echo');
    await click('models-echo-panel-delete');
    await click('models-echo-confirm-delete');
    expect(q('models-echo-panel-error')?.textContent).toContain(
      "part of qrtz's shared speech",
    );
    expect(el.textContent).not.toContain('remove it in opts');
  });

  it('switching an app on downloads nothing: its models came with its install (ADR 0169)', async () => {
    const f = fake({ 'speech:status': () => missing });
    await render(f.client);
    await clickRow('echo');
    await click('module-enabled-echo');
    await click('module-enabled-echo');
    expect(f.calls.map((c) => c.channel)).not.toContain('speech:download');
  });
});

describe('shared models and services', () => {
  // Jared, 2026-10-18: the shared section said "used by nothing" for the four speech models while clean up said
  // there was nothing to clean up. They are the models the app pinned for echo and nano (workspace requests, no
  // extension row in qrtz.lock), and the label now reads the host's usedBy, the answer prune keeps by.
  it('names echo and nano as the users of their pinned models and offers no clean up', async () => {
    const pinned = (name: string, sizeBytes: number, usedBy: string[]) => ({
      name,
      version: '1.0.0',
      kind: 'model' as const,
      requested: '=1.0.0',
      requiredBy: [],
      usedBy,
      disabled: null,
      sizeBytes,
    });
    const f = fake({
      'packages:list': () => ({
        ...list,
        packages: [
          pinned('model.localvqe', 20_000_000, ['nano']),
          pinned('model.nemotron-diarizer', 199_000_000, ['nano']),
          pinned('model.nemotron', 626_000_000, ['nano']),
          pinned('model.parakeet-tdt-v2-en', 464_000_000, ['echo']),
        ],
        reclaimable: { dirs: 0, blobs: 0, bytes: 0 },
      }),
    });
    await render(f.client);
    const shared = q('module-shared')?.textContent ?? '';
    expect(shared).not.toContain('used by nothing');
    expect(shared).toContain('464 mb · used by echo');
    expect(shared).toContain('626 mb · used by nano');
    expect(shared).toContain('199 mb · used by nano');
    expect(shared).toContain('20 mb · used by nano');
    expect(q('module-prune')).toBeNull();
    expect(q('module-prune-none')?.textContent).toContain('in use');
  });
});

describe('qrtz ships empty (ADR 0169)', () => {
  const none = (): Record<string, (p: unknown) => unknown> => {
    const cfg = defaultConfig();
    for (const id of ['echo', 'nano', 'ctrl', 'snap'] as const)
      cfg.modules[id].installed = false;
    return {
      'config:get': () => cfg,
      'packages:list': () => ({
        packages: [],
        index: null,
        available: [],
        reclaimable: { dirs: 0, blobs: 0, bytes: 0 },
      }),
    };
  };

  it('says qrtz starts empty: nothing installed, and a check for apps', async () => {
    const f = fake(none());
    await render(f.client);
    for (const n of ['echo', 'nano', 'ctrl', 'snap'])
      expect(q(`app-row-${n}`), n).toBeNull();
    expect(q('installed-empty')?.textContent).toContain(
      'nothing installed yet',
    );
    expect(el.textContent).toContain('qrtz starts empty');
    await click('marketplace-empty-check');
    expect(f.calls.map((c) => c.channel)).toContain('packages:refresh');
    expect(el.textContent).toBe(el.textContent?.toLowerCase());
  });
});

describe('every permission is granted from inside qrtz (in each app modal)', () => {
  const wait = (ms: number) =>
    act(async () => {
      await new Promise((r) => setTimeout(r, ms));
    });

  it("shows a switched-on app's permissions with their state and asks only on allow", async () => {
    const f = fake({
      'permissions:get': () => ({
        microphone: 'not-determined',
        accessibility: 'granted',
        'screen-recording': 'denied',
      }),
      'permissions:request': () => ({
        microphone: 'granted',
        accessibility: 'granted',
        'screen-recording': 'denied',
      }),
    });
    await render(f.client);
    await clickRow('echo');
    expect(q('perm-echo-microphone')?.textContent).toContain('allow');
    expect(q('perm-echo-accessibility')?.textContent).toContain('allowed');
    expect(f.calls.map((c) => c.channel)).not.toContain('permissions:request');
    await act(async () =>
      q('perm-echo-microphone')!.querySelector('button')!.click(),
    );
    await settle();
    expect(f.calls.filter((c) => c.channel === 'permissions:request')).toEqual([
      { channel: 'permissions:request', payload: { permission: 'microphone' } },
    ]);
    expect(q('perm-echo-microphone')?.textContent).toContain('allowed');
    await click('app-modal-close');
    await clickRow('nano');
    expect(
      q('perm-nano-screen-recording')?.querySelector('button'),
    ).not.toBeNull();
    await click('app-modal-close');
    await clickRow('ctrl');
    // ADR 0214 review: ctrl asks for accessibility only.
    expect(q('perm-ctrl-accessibility')).not.toBeNull();
    expect(q('perm-ctrl-automation')).toBeNull();
    // An app that is off shows no rows.
    await click('module-enabled-ctrl');
    expect(q('perm-ctrl-accessibility')).toBeNull();
  });

  it('allow asks qrtz, says "switch on qrtz", and flips to allowed by itself once switched on', async () => {
    let ax = 'not-determined';
    const perms = () => ({
      microphone: 'granted',
      accessibility: ax,
      'screen-recording': 'granted',
    });
    const f = fake({
      'permissions:get': perms,
      'permissions:request': perms,
    });
    await render(f.client);
    await clickRow('echo');
    const row = () => q('perm-echo-accessibility');
    await act(async () => row()!.querySelector('button')!.click());
    await settle();
    expect(f.calls.filter((c) => c.channel === 'permissions:request')).toEqual([
      {
        channel: 'permissions:request',
        payload: { permission: 'accessibility' },
      },
    ]);
    expect(q('perm-echo-accessibility-hint')?.textContent).toBe(
      'switch on qrtz',
    );
    ax = 'granted';
    await wait(1100);
    expect(row()?.textContent).toContain('allowed');
    expect(q('perm-echo-accessibility-hint')).toBeNull();
  });

  it('screen recording offers "restart qrtz" after asking', async () => {
    const perms = () => ({
      microphone: 'granted',
      accessibility: 'granted',
      'screen-recording': 'not-determined',
    });
    const f = fake({
      'permissions:get': perms,
      'permissions:request': perms,
    });
    await render(f.client);
    await clickRow('nano');
    expect(q('perm-nano-screen-recording-restart')).toBeNull();
    await act(async () =>
      q('perm-nano-screen-recording')!.querySelector('button')!.click(),
    );
    await settle();
    await click('perm-nano-screen-recording-restart');
    expect(f.calls.map((c) => c.channel)).toContain('app:restart');
    expect(el.textContent).toBe(el.textContent?.toLowerCase());
  });

  it('says "restart to finish updating" for an app an update left running its old version (ADR 0172)', async () => {
    const g = fake({
      'packages:list': () => ({ ...list, pendingRestart: ['nano'] }),
    });
    await render(g.client);
    expect(q('marketplace-restart')?.textContent).toContain(
      'restart to finish updating',
    );
    expect(q('marketplace-restart')?.textContent).toContain('nano');
    expect(q('app-status-nano')?.textContent).toBe('restart to update');
    await click('marketplace-restart-button');
    expect(g.calls.map((c) => c.channel)).toContain('app:restart');
    expect(el.textContent).toBe(el.textContent?.toLowerCase());
  });
});

describe('the store unreachable or empty (every host failed)', () => {
  for (const raw of [
    'every host failed: https://modules.qrtz.ai/v1/timestamp.json: not found',
    'every host failed: modules.qrtz.ai: error sending request',
  ])
    it(`shows a friendly retry, not the raw E_UNAVAILABLE text, and logs it (${raw.slice(19, 40)}…)`, async () => {
      let down = true;
      const f = fake({
        'packages:refresh': () => {
          if (down)
            throw new AppError('E_UNAVAILABLE', raw, { retryable: true });
          return ipcSamples['packages:refresh'].response;
        },
        'log:write': () => ({ accepted: true }),
      });
      await render(f.client);
      await click('marketplace-check');
      expect(q('marketplace-error')).toBeNull();
      const box = q('marketplace-unavailable');
      expect(box?.textContent).toContain(
        "the store isn't available right now, try again in a bit",
      );
      expect(el.textContent).not.toContain('every host failed');
      expect(el.textContent).not.toContain('E_UNAVAILABLE');
      expect(
        f.calls.filter((c) => c.channel === 'log:write').map((c) => c.payload),
      ).toEqual([
        {
          level: 'warn',
          scope: 'marketplace',
          message: `store unavailable: ${raw}`,
        },
      ]);
      // retry asks again; once the store answers, the message goes away.
      down = false;
      await click('marketplace-retry');
      expect(
        f.calls.filter((c) => c.channel === 'packages:refresh'),
      ).toHaveLength(2);
      expect(q('marketplace-unavailable')).toBeNull();
    });
});

describe('rows', () => {
  it("bundled echo's row shows the app version and 'built in', not 'vinstalled · on this mac'", async () => {
    const f = fake({
      'packages:list': () => ({
        ...list,
        available: list.available.filter((e) => e.name !== 'echo'),
        packages: list.packages.filter((r) => r.name !== 'echo'),
      }),
      'app:get-info': () => ({
        ...ipcSamples['app:get-info'].response,
        version: '0.2.0',
      }),
    });
    await render(f.client);
    expect(q('app-meta-echo')?.textContent).toBe('v0.2.0 · built in');
    expect(q('app-status-echo')?.textContent).toBe('built in');
    expect(el.textContent).not.toContain('vinstalled');
  });

  it('are one short line each: glyph, name, meta and one line of description, the action on the right', async () => {
    await render(fake().client);
    const r = q('app-row-echo')!;
    expect(r.tagName).toBe('LI');
    expect(r.closest('ul')?.hasAttribute('data-app-list')).toBe(true);
    const grid = r.firstElementChild as HTMLElement;
    expect(grid.style.display).toBe('grid');
    expect(grid.style.gridTemplateColumns).toBe('minmax(0, 1fr) auto auto');
    expect(grid.textContent).toContain('dictation anywhere');
    expect(grid.querySelector('[data-testid="app-menu-echo"]')).not.toBeNull();
  });
});

describe('user-facing words', () => {
  const words = (t: string | null | undefined) =>
    (t ?? '')
      .replaceAll('modules.qrtz.ai', '')
      .match(/\b(modules?|extensions?)\b/gi);

  it("say 'apps', never 'module' or 'extension', on the page and in a modal", async () => {
    await render(fake().client);
    expect(words(el.textContent)).toBeNull();
    expect(el.textContent).toContain('installed');
    expect(el.textContent).toContain('available');
    await clickRow('echo');
    expect(words(q('app-modal')?.textContent)).toBeNull();
  });
});
