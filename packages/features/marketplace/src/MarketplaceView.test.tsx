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
          modules: { ...config.modules, [name]: { enabled } },
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
    // echo is installed (open), nano and ctrl ship in the build (open).
    expect(q('extension-launch-echo')).not.toBeNull();
    expect(q('extension-card-echo')?.textContent).toContain(
      '629 mb with dependencies',
    );
    expect(q('extension-card-ctrl')?.textContent).toContain('built in');
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

  it('lists installed and built-in extensions with their dependencies', async () => {
    await open();
    expect(q('module-manager')).not.toBeNull();
    for (const n of ['echo', 'nano', 'ctrl'])
      expect(q(`module-row-${n}`), n).not.toBeNull();
    expect(q('module-deps-echo')?.textContent).toContain(
      'model.parakeet-tdt-v2-en',
    );
    expect(q('module-shared')?.textContent).toContain('used by echo');
    // Only a package can be removed; a built-in module is switched off instead.
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
    const f = await open();
    await click('module-remove-echo');
    expect(f.calls.find((c) => c.channel === 'packages:plan')?.payload).toEqual(
      { op: 'remove', names: ['echo'] },
    );
    expect(q('install-consent')).not.toBeNull();
    await click('consent-confirm');
    expect(f.calls.map((c) => c.channel)).toContain('packages:apply');
    await click('module-prune');
    expect(f.calls.map((c) => c.channel)).toContain('packages:prune');
  });
});
