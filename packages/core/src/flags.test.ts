import { afterEach, describe, expect, it, vi } from 'vitest';

const flags = async () => {
  vi.resetModules();
  return import('./flags');
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('module flags', () => {
  it('outside a Tauri window: hides nothing and never calls PostHog', async () => {
    const fetch = vi.fn();
    vi.stubGlobal('fetch', fetch);
    const f = await flags();
    expect(f.hiddenModules()).toEqual([]);
    expect(await f.loadHiddenModules()).toEqual([]);
    expect(fetch).not.toHaveBeenCalled();
  });

  it('in a Tauri window: hides every flagged module until PostHog answers, then only the ones off', async () => {
    const store = new Map<string, string>();
    vi.stubGlobal('window', { __TAURI_INTERNALS__: {} });
    vi.stubGlobal('localStorage', {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => store.set(k, v),
    });
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({
        ok: true,
        json: async () => ({ flags: { 'app-ctrl': { enabled: true } } }),
      })),
    );
    const f = await flags();
    expect(f.hiddenModules()).toEqual(['nano', 'ctrl']);
    expect(await f.loadHiddenModules()).toEqual(['nano']);

    // Offline next launch: the last answer stands.
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
    expect(await (await flags()).loadHiddenModules()).toEqual(['nano']);
  });
});
