// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import type { QrtzClient, SpeechSetupStatus } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { ipcSamples } from '@qrtz/core/ipc/samples';
import { ThemeProvider } from '@qrtz/ui';
import TalkView from './TalkView';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const none: SpeechSetupStatus = {
  configured: true,
  installed: null,
  bytes: 0,
  latest: '1.0.0',
  latestBytes: 628_000_000,
  waiting: null,
  listExpired: false,
  download: { state: 'idle', done: 0, total: 0 },
};
const downloading: SpeechSetupStatus = {
  ...none,
  download: { state: 'downloading', done: 1, total: 628_000_000 },
};
const installed: SpeechSetupStatus = {
  ...none,
  installed: '1.0.0',
  bytes: 628_000_000,
};

type Call = { channel: string; req: unknown };

/** echo's view with a fresh config (`{}`: every module on) and `speech:status` answered by `status()`. */
async function mount(
  status: () => Promise<SpeechSetupStatus>,
  initial: Record<string, unknown> = {},
  /** The unscoped `speech:status` (the speech notice's); the samples' "installed" when absent. */
  globalStatus?: () => Promise<SpeechSetupStatus>,
) {
  const calls: Call[] = [];
  // Several hooks listen to the same event (echo's switch and the speech notice both follow `speech:status`).
  const listeners = new Map<string, Set<(p: unknown) => void>>();
  let cfg = initial;
  const client = {
    subApp: 'talk',
    invoke: async (channel: string, req: unknown) => {
      calls.push({ channel, req });
      if (channel === 'config:get') return cfg;
      if (channel === 'config:set') {
        cfg = { ...cfg, ...(req as { patch: object }).patch };
        return cfg;
      }
      if (
        channel === 'speech:status' &&
        (req as { extensions?: string[] })?.extensions
      )
        return status();
      if (channel === 'speech:status' && globalStatus) return globalStatus();
      if (channel === 'speech:download') return downloading;
      return (ipcSamples as Record<string, { response: unknown }>)[channel]
        ?.response;
    },
    on: (event: string, fn: (p: unknown) => void) => {
      const set = listeners.get(event) ?? new Set();
      set.add(fn);
      listeners.set(event, set);
      return () => set.delete(fn);
    },
  } as unknown as QrtzClient;
  const el = document.createElement('div');
  document.body.appendChild(el);
  const root = createRoot(el);
  await act(async () =>
    root.render(
      <ThemeProvider mode="light">
        <QrtzClientProvider client={client}>
          <TalkView />
        </QrtzClientProvider>
      </ThemeProvider>,
    ),
  );
  const settle = () =>
    act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
  await settle();
  const q = (id: string) =>
    el.querySelector<HTMLElement>(`[data-testid="${id}"]`);
  const emit = (event: string, p: unknown) =>
    act(async () => listeners.get(event)?.forEach((fn) => fn(p)));
  return { calls, q, settle, emit, unmount: () => root.unmount() };
}

describe("echo's view offers parakeet in place (ADR 0175)", () => {
  it('a fresh qrtz has echo on and offers the download without touching the switch', async () => {
    const v = await mount(async () => none);
    expect(v.q('talk')).not.toBeNull();
    expect(v.q('echo-switch')?.getAttribute('aria-checked')).toBe('true');
    const offer = v.q('echo-offer');
    expect(offer).not.toBeNull();
    // The standard model view: name, real size, and where it comes from (all lowercase).
    expect(offer!.textContent).toContain('speech model');
    expect(offer!.textContent).toContain('not downloaded · 628 mb');
    expect(offer!.textContent).toContain('downloads once from hugging face');
    expect(v.q('echo-download')?.textContent).toBe('download');
    await act(async () => v.q('echo-download')!.click());
    await v.settle();
    expect(
      v.calls.filter((c) => c.channel === 'speech:download').map((c) => c.req),
    ).toEqual([{ extensions: ['echo'] }]);
    // The model is required: the gate stays until it's installed, and nothing touched the config.
    expect(v.q('echo-offer')).not.toBeNull();
    expect(v.q('echo-offer-blocked')?.hasAttribute('inert')).toBe(true);
    expect(v.calls.some((c) => c.channel === 'config:set')).toBe(false);
    v.unmount();
  });

  it('shows only the offer, not also the speech notice with its settings link, until a download starts', async () => {
    const v = await mount(
      async () => none,
      {},
      async () => none,
    );
    expect(v.q('echo-offer')).not.toBeNull();
    expect(v.q('speech-notice')).toBeNull();
    expect(v.q('speech-notice-open')).toBeNull();
    await act(async () => v.q('echo-download')!.click());
    await v.emit('speech:status', downloading);
    await v.settle();
    // Downloading: the same gate shows the progress bar, still with no separate notice.
    expect(v.q('echo-offer')?.dataset.state).toBe('downloading');
    expect(v.q('echo-offer')?.textContent).toContain('downloading · 0%');
    expect(v.q('speech-notice')).toBeNull();
    v.unmount();
  });

  it('a failed download shows its reason in the offer, which still replaces the notice', async () => {
    const failed: SpeechSetupStatus = {
      ...none,
      download: { state: 'error', done: 0, total: 0, error: 'network' },
    };
    const v = await mount(
      async () => failed,
      {},
      async () => failed,
    );
    expect(v.q('echo-offer')?.textContent).toContain(
      "couldn't download · network",
    );
    expect(v.q('speech-notice')).toBeNull();
    v.unmount();
  });

  it('blocks with the progress while a download runs, and lifts once parakeet is installed', async () => {
    let s = downloading;
    const v = await mount(async () => s);
    expect(v.q('talk')).not.toBeNull();
    expect(v.q('echo-offer')?.dataset.state).toBe('downloading');
    expect(v.q('echo-download')).toBeNull();
    // The download finishes: the view reads echo's status again and the gate lifts.
    s = installed;
    await v.emit('speech:status', installed);
    await v.settle();
    expect(v.q('echo-offer')).toBeNull();
    expect(
      v.calls.filter(
        (c) =>
          c.channel === 'speech:status' &&
          (c.req as { extensions?: string[] })?.extensions,
      ).length,
    ).toBeGreaterThan(1);
    v.unmount();
  });

  it('offers again when a download fails', async () => {
    const v = await mount(async () => ({
      ...none,
      download: { state: 'error', done: 0, total: 0, error: 'network' },
    }));
    expect(v.q('echo-offer')).not.toBeNull();
    v.unmount();
  });

  it('offers nothing while echo is off (the switch asks instead)', async () => {
    const v = await mount(async () => none);
    await act(async () => v.q('echo-switch')!.click());
    await v.settle();
    expect(v.q('talk-off')).not.toBeNull();
    expect(v.q('echo-offer')).toBeNull();
    v.unmount();
  });
});

describe("echo's speech notice checks only echo's models (QRT-277)", () => {
  it("says nothing when parakeet is installed but nano's models are not", async () => {
    // The global status covers every switched-on module (nano's Nemotron sets too): not installed.
    const v = await mount(
      async () => installed,
      {},
      async () => none,
    );
    expect(v.q('echo-offer')).toBeNull();
    expect(v.q('speech-notice')).toBeNull();
    // A streamed (global) status event doesn't bring the notice back either.
    await v.emit('speech:status', none);
    await v.settle();
    expect(v.q('speech-notice')).toBeNull();
    expect(
      v.calls.some(
        (c) =>
          c.channel === 'speech:status' &&
          !(c.req as { extensions?: string[] })?.extensions,
      ),
    ).toBe(false);
    v.unmount();
  });
});

describe('the echo switch waits for the model status (N180)', () => {
  const deferred = () => {
    let answer: (s: SpeechSetupStatus) => void = () => {};
    const pending = new Promise<SpeechSetupStatus>((r) => (answer = r));
    return { pending, answer: (s: SpeechSetupStatus) => answer(s) };
  };

  it('is disabled once config loads but before speech:status returns, then enabled', async () => {
    const d = deferred();
    const v = await mount(() => d.pending);
    const sw = () => v.q('echo-switch') as HTMLButtonElement;
    expect(sw().getAttribute('aria-checked')).toBe('true');
    expect(sw().disabled).toBe(true);
    await act(async () => d.answer(none));
    await v.settle();
    expect(sw().disabled).toBe(false);
    v.unmount();
  });

  it('a click on a switched-off echo in that gap neither turns it on nor downloads', async () => {
    const d = deferred();
    const v = await mount(() => d.pending, {
      modules: { echo: { enabled: false } },
    });
    expect(v.q('talk-off')).not.toBeNull();
    const sw = () => v.q('echo-switch') as HTMLButtonElement;
    expect(sw().disabled).toBe(true);
    await act(async () => sw().click());
    await v.settle();
    expect(
      v.calls.some(
        (c) => c.channel === 'config:set' || c.channel === 'speech:download',
      ),
    ).toBe(false);
    // Once the status is in, the same click asks for consent first.
    await act(async () => d.answer(none));
    await v.settle();
    await act(async () => sw().click());
    await v.settle();
    expect(document.body.textContent).toContain('download and turn on');
    expect(v.calls.some((c) => c.channel === 'config:set')).toBe(false);
    v.unmount();
  });
});

describe('a failed speech:status (N183)', () => {
  it('says so with a retry, and leaves the switch usable through the consent dialog', async () => {
    let fail = true;
    const v = await mount(
      () =>
        fail
          ? Promise.reject(new Error('E_UNAVAILABLE'))
          : Promise.resolve(none),
      { modules: { echo: { enabled: false } } },
    );
    const sw = () => v.q('echo-switch') as HTMLButtonElement;
    expect(v.q('echo-status-error')?.textContent).toContain(
      "couldn't check speech, retry",
    );
    // Not dead: the switch is enabled, and turning echo on asks first (the model's state is unknown).
    expect(sw().disabled).toBe(false);
    await act(async () => sw().click());
    await v.settle();
    expect(document.body.textContent).toContain('download and turn on');
    expect(v.calls.some((c) => c.channel === 'config:set')).toBe(false);
    // retry reads the status again; once it answers, the error goes away.
    fail = false;
    await act(async () => v.q('echo-status-retry')!.click());
    await v.settle();
    expect(v.q('echo-status-error')).toBeNull();
    expect(
      v.calls.filter(
        (c) =>
          c.channel === 'speech:status' &&
          (c.req as { extensions?: string[] })?.extensions,
      ).length,
    ).toBeGreaterThanOrEqual(2);
    v.unmount();
  });

  it('on the on page, the error shows above the gate (an unread status is not ready) and the switch still works', async () => {
    const v = await mount(() => Promise.reject(new Error('E_UNAVAILABLE')));
    expect(v.q('talk')).not.toBeNull();
    expect(v.q('echo-status-error')).not.toBeNull();
    expect(v.q('echo-offer')?.dataset.state).toBe('missing');
    expect((v.q('echo-switch') as HTMLButtonElement).disabled).toBe(false);
    v.unmount();
  });
});

describe('turning echo on after a failed speech:status (N184)', () => {
  const echoStatus = (v: { calls: Call[] }) =>
    v.calls.filter(
      (c) =>
        c.channel === 'speech:status' &&
        (c.req as { extensions?: string[] })?.extensions,
    ).length;

  it('asks speech:status again first; installed Parakeet turns echo on with no consent and no download', async () => {
    let fail = true;
    const v = await mount(
      () =>
        fail
          ? Promise.reject(new Error('E_UNAVAILABLE'))
          : Promise.resolve(installed),
      { modules: { echo: { enabled: false } } },
    );
    expect(v.q('echo-status-error')).not.toBeNull();
    const before = echoStatus(v);
    fail = false;
    await act(async () => (v.q('echo-switch') as HTMLButtonElement).click());
    await v.settle();
    expect(echoStatus(v)).toBeGreaterThan(before);
    expect(v.calls.find((c) => c.channel === 'config:set')?.req).toEqual({
      patch: { modules: { echo: { enabled: true } } },
    });
    expect(document.body.textContent).not.toContain('download and turn on');
    expect(v.q('echo-consent')).toBeNull();
    expect(v.calls.map((c) => c.channel)).not.toContain('speech:download');
    v.unmount();
  });

  it('asks for consent when the second answer says parakeet is missing', async () => {
    let fail = true;
    const v = await mount(
      () =>
        fail
          ? Promise.reject(new Error('E_UNAVAILABLE'))
          : Promise.resolve(none),
      { modules: { echo: { enabled: false } } },
    );
    fail = false;
    await act(async () => (v.q('echo-switch') as HTMLButtonElement).click());
    await v.settle();
    expect(document.body.textContent).toContain('download and turn on');
    expect(v.calls.some((c) => c.channel === 'config:set')).toBe(false);
    expect(v.calls.map((c) => c.channel)).not.toContain('speech:download');
    v.unmount();
  });
});

describe('echo on its bundled copy', () => {
  it('shows a small lowercase notice when the host reports the fallback, on and off', async () => {
    for (const enabled of [true, false]) {
      const v = await mount(async () => installed, {
        modules: { echo: { enabled, bundledFallback: true } },
      });
      const n = v.q('echo-bundled-notice');
      expect(n?.textContent).toBe(
        "echo is using the copy built into qrtz: its update couldn't start",
      );
      expect(n?.textContent).toBe(n?.textContent?.toLowerCase());
      v.unmount();
    }
  });

  it('shows nothing without it, and the consent says mb', async () => {
    const v = await mount(async () => installed);
    expect(v.q('echo-bundled-notice')).toBeNull();
    expect(document.body.textContent).not.toMatch(/\bMB\b/);
    v.unmount();
  });
});
