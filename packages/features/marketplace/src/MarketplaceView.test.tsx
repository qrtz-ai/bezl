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
import { ThemeProvider, ToastProvider } from '@qrtz/ui';
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

describe('marketplace view (ADR 0161)', () => {
  it('browses every extension as a card with its size and button, without going online', async () => {
    const f = fake();
    await render(f.client);
    for (const n of ['echo', 'nano', 'ctrl'])
      expect(q(`extension-card-${n}`), n).not.toBeNull();
    // echo is installed from the index (open); nano and ctrl count as installed here (the fake's config, like
    // the mock backend, says every module is) and list without an index entry.
    expect(q('extension-launch-echo')).not.toBeNull();
    expect(q('extension-card-echo')?.textContent).toContain(
      '629 mb with dependencies',
    );
    expect(q('extension-card-ctrl')?.textContent).toContain('installed');
    expect(el.textContent).not.toContain('built in');
    expect(f.calls.map((c) => c.channel)).not.toContain('packages:refresh');
    expect(el.textContent).toBe(el.textContent?.toLowerCase());
  });

  it('checks for extensions only when asked', async () => {
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
          "marketplace coming soon. modules will appear here once qrtz.ai's module server is live.",
          { retryable: false, details: { reason: 'not-set-up' } },
        );
      },
    });
    await render(f.client);
    await click('marketplace-check');
    expect(q('marketplace-error')).toBeNull();
    const soon = q('marketplace-soon');
    expect(soon?.textContent).toContain('marketplace coming soon');
    expect(soon?.textContent).toContain(
      "modules will appear here once qrtz.ai's module server is live",
    );
    expect(soon?.textContent).not.toContain('root');
    // The installed extensions still list.
    expect(q('extension-card-echo')).not.toBeNull();
  });

  it('installs through a plan the user confirms, with progress', async () => {
    // A new extension this build doesn't ship: available until installed.
    const empty = { ...ipcSamples['packages:list'].response, packages: [] };
    const weather = {
      ...empty.available[0]!,
      name: 'weather',
      permissions: [],
    };
    let release = () => {};
    const g = fake({
      'packages:list': () => ({ ...empty, available: [weather] }),
      'packages:apply': () =>
        new Promise((r) => {
          release = () => r(ipcSamples['packages:apply'].response);
        }),
    });
    await render(g.client);
    expect(q('extension-card-weather')?.textContent).toContain('available');
    await click('extension-install-weather');
    expect(g.calls.find((c) => c.channel === 'packages:plan')?.payload).toEqual(
      { op: 'install', names: ['weather'] },
    );
    // Nothing applies until the consent is confirmed; the consent names the download.
    expect(g.calls.map((c) => c.channel)).not.toContain('packages:apply');
    expect(q('consent-download')?.textContent).toContain('629 mb');
    expect(q('consent-changes')?.textContent).toContain('add echo');
    await click('consent-confirm');
    const apply = g.calls.find((c) => c.channel === 'packages:apply');
    expect(apply?.payload).toEqual({
      planSha256: ipcSamples['packages:plan'].response.planSha256,
      downloadBytes: ipcSamples['packages:plan'].response.downloadBytes,
    });
    await act(async () =>
      g.push('packages:progress', {
        doneBytes: 314_000_000,
        totalBytes: 628_500_000,
      }),
    );
    expect(q('install-progress')?.textContent).toContain('314 mb of 629 mb');
    expect(q('extension-progress-weather')?.getAttribute('aria-valuenow')).toBe(
      '50',
    );
    await act(async () => release());
    await settle();
    expect(q('install-progress')).toBeNull();
    // The list is read again after the install.
    expect(
      g.calls.filter((c) => c.channel === 'packages:list').length,
    ).toBeGreaterThan(1);
  });

  it('shows a detail page: version, install size, permissions and dependencies', async () => {
    const f = fake();
    await render(f.client);
    await click('extension-open-echo');
    expect(q('marketplace-item')).not.toBeNull();
    expect(q('fact-version')?.textContent).toBe('v0.3.0');
    expect(q('fact-install-size')?.textContent).toContain('629 mb');
    expect(q('permission-microphone')).not.toBeNull();
    expect(q('permission-accessibility')).not.toBeNull();
    expect(q('dep-inference.voice')?.textContent).toContain('service');
    expect(q('dep-model.parakeet-tdt-v2-en')?.textContent).toContain(
      'via a dependency',
    );
    await click('marketplace-back');
    expect(q('marketplace-grid')).not.toBeNull();
  });
});

describe('module manager (ADR 0161)', () => {
  const open = async () => {
    const f = fake();
    await render(f.client);
    await act(async () => q('tab-installed')!.click());
    await settle();
    return f;
  };

  it('lists the installed extensions with their dependencies', async () => {
    await open();
    expect(q('module-manager')).not.toBeNull();
    for (const n of ['echo', 'nano', 'ctrl'])
      expect(q(`module-row-${n}`), n).not.toBeNull();
    expect(q('module-deps-echo')?.textContent).toContain(
      'model.parakeet-tdt-v2-en',
    );
    expect(q('module-shared')?.textContent).toContain('used by echo');
    // Only a package in the lock can be removed (ctrl here has no lock row: the fake's config says installed).
    expect(q('module-remove-echo')).not.toBeNull();
    expect(q('module-remove-ctrl')).toBeNull();
  });

  it('switches a module off through packages:set-enabled only', async () => {
    const f = await open();
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
    // Its settings can't open while it is off.
    expect(
      (q('module-settings-ctrl') as HTMLButtonElement | null)?.disabled,
    ).toBe(true);
  });

  it('opens a module own settings in its view', async () => {
    await open();
    const seen: NavRequest[] = [];
    const off = onNavigateRequest((r) => seen.push(r));
    await click('module-settings-echo');
    off();
    expect(seen).toEqual([{ app: 'talk', settings: true }]);
  });

  it('removes through a plan and prunes', async () => {
    // After the remove, the host's dry run of prune finds echo's old files: only then is clean up offered.
    const sample = ipcSamples['packages:list'].response;
    let removed = false;
    const f = fake({
      'packages:list': () =>
        removed
          ? {
              ...sample,
              packages: [],
              reclaimable: { dirs: 3, blobs: 4, bytes: 628_500_000 },
            }
          : sample,
      'packages:apply': () => {
        removed = true;
        return ipcSamples['packages:apply'].response;
      },
    });
    await render(f.client);
    await act(async () => q('tab-installed')!.click());
    await settle();
    expect(q('module-prune')).toBeNull();
    await click('module-remove-echo');
    expect(f.calls.find((c) => c.channel === 'packages:plan')?.payload).toEqual(
      { op: 'remove', names: ['echo'] },
    );
    expect(q('install-consent')).not.toBeNull();
    await click('consent-confirm');
    expect(f.calls.map((c) => c.channel)).toContain('packages:apply');
    expect(q('module-prune')?.textContent).toContain('629 mb');
    await click('module-prune');
    expect(f.calls.map((c) => c.channel)).toContain('packages:prune');
  });

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
        ...ipcSamples['packages:list'].response,
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
    await act(async () => q('tab-installed')!.click());
    await settle();
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

describe('no onboarding: permissions and the speech yes live at the switch (ADR 0163)', () => {
  const missing = {
    ...ipcSamples['speech:status'].response,
    installed: null,
    bytes: 0,
    download: { state: 'idle', done: 0, total: 0 },
  };
  const open = async (overrides: Record<string, (p: unknown) => unknown>) => {
    const f = fake(overrides);
    await render(f.client);
    await act(async () => q('tab-installed')!.click());
    await settle();
    return f;
  };

  it("shows each switched-on module's permissions with their state and asks only on allow", async () => {
    const f = await open({
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
    expect(q('perm-echo-microphone')?.textContent).toContain('allow');
    expect(q('perm-echo-accessibility')?.textContent).toContain('allowed');
    expect(
      q('perm-nano-screen-recording')?.querySelector('button'),
    ).not.toBeNull();
    expect(q('perm-ctrl-automation')?.textContent).toContain(
      'asked on first use',
    );
    const channels = () => f.calls.map((c) => c.channel);
    for (const ch of ['permissions:request', 'speech:download'])
      expect(channels(), ch).not.toContain(ch);
    await act(async () =>
      q('perm-echo-microphone')!.querySelector('button')!.click(),
    );
    await settle();
    expect(f.calls.filter((c) => c.channel === 'permissions:request')).toEqual([
      { channel: 'permissions:request', payload: { permission: 'microphone' } },
    ]);
    expect(q('perm-echo-microphone')?.textContent).toContain('allowed');
    expect(q('perm-nano-microphone')?.textContent).toContain('allowed');
    // A module that is off shows no rows.
    await click('module-enabled-ctrl');
    expect(q('perm-ctrl-accessibility')).toBeNull();
  });

  it('switching a module on downloads nothing: its models came with its install (ADR 0169)', async () => {
    const f = await open({ 'speech:status': () => missing });
    await click('module-enabled-echo');
    await click('module-enabled-echo');
    await click('module-enabled-ctrl');
    await click('module-enabled-ctrl');
    expect(f.calls.map((c) => c.channel)).not.toContain('speech:download');
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

  it('browse says qrtz starts empty and offers the check', async () => {
    const f = fake(none());
    await render(f.client);
    for (const n of ['echo', 'nano', 'ctrl', 'snap'])
      expect(q(`extension-card-${n}`), n).toBeNull();
    expect(el.textContent).toContain('qrtz starts empty');
    await click('marketplace-empty-check');
    expect(f.calls.map((c) => c.channel)).toContain('packages:refresh');
    expect(el.textContent).toBe(el.textContent?.toLowerCase());
  });

  it('the module manager has nothing installed and points to browse', async () => {
    const f = fake(none());
    await render(f.client);
    await act(async () => q('tab-installed')!.click());
    await settle();
    for (const n of ['echo', 'nano', 'ctrl', 'snap'])
      expect(q(`module-row-${n}`), n).toBeNull();
    expect(q('module-list')?.textContent).toContain('nothing installed yet');
    await click('module-list-browse');
    expect(q('marketplace')).not.toBeNull();
  });

  it('an index module is available, not built in, until installed', async () => {
    const list = ipcSamples['packages:list'].response;
    const f = fake({
      ...none(),
      'packages:list': () => ({ ...list, packages: [] }),
    });
    await render(f.client);
    expect(q('extension-card-echo')?.textContent).toContain('available');
    expect(q('extension-install-echo')).not.toBeNull();
    expect(q('extension-launch-echo')).toBeNull();
  });
});

describe('every permission is granted from inside qrtz', () => {
  const tab = async (overrides: Record<string, (p: unknown) => unknown>) => {
    const f = fake(overrides);
    await render(f.client);
    await act(async () => q('tab-installed')!.click());
    await settle();
    return f;
  };
  const wait = (ms: number) =>
    act(async () => {
      await new Promise((r) => setTimeout(r, ms));
    });

  it('allow asks qrtz, says "switch on qrtz", and flips to allowed by itself once switched on', async () => {
    let ax = 'not-determined';
    const perms = () => ({
      microphone: 'granted',
      accessibility: ax,
      'screen-recording': 'granted',
    });
    const f = await tab({
      'permissions:get': perms,
      'permissions:request': perms,
    });
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

  it('screen recording offers "restart qrtz" after asking; automation opens its own pane', async () => {
    const perms = () => ({
      microphone: 'granted',
      accessibility: 'granted',
      'screen-recording': 'not-determined',
    });
    const f = await tab({
      'permissions:get': perms,
      'permissions:request': perms,
    });
    expect(q('perm-nano-screen-recording-restart')).toBeNull();
    await act(async () =>
      q('perm-nano-screen-recording')!.querySelector('button')!.click(),
    );
    await settle();
    await click('perm-nano-screen-recording-restart');
    expect(f.calls.map((c) => c.channel)).toContain('app:restart');
    await click('perm-ctrl-automation-open');
    expect(
      f.calls.find((c) => c.channel === 'permissions:open-settings')?.payload,
    ).toEqual({ pane: 'automation' });
    expect(el.textContent).toBe(el.textContent?.toLowerCase());
  });

  it('says "restart to finish updating" for a module an update left running its old version (ADR 0172)', async () => {
    const f = fake();
    await render(f.client);
    expect(q('marketplace-restart')).toBeNull();
    const list = (ipcSamples as Record<string, { response: object }>)[
      'packages:list'
    ].response;
    const g = fake({
      'packages:list': () => ({ ...list, pendingRestart: ['nano'] }),
    });
    act(() => root?.unmount());
    el.remove();
    await render(g.client);
    expect(q('marketplace-restart')?.textContent).toContain(
      'restart to finish updating',
    );
    expect(q('marketplace-restart')?.textContent).toContain('nano');
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
