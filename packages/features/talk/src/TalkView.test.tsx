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
) {
  const calls: Call[] = [];
  const listeners = new Map<string, (p: unknown) => void>();
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
      if (channel === 'speech:download') return downloading;
      return (ipcSamples as Record<string, { response: unknown }>)[channel]
        ?.response;
    },
    on: (event: string, fn: (p: unknown) => void) => {
      listeners.set(event, fn);
      return () => listeners.delete(event);
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
    act(async () => listeners.get(event)?.(p));
  return { calls, q, settle, emit, unmount: () => root.unmount() };
}

describe("echo's view offers parakeet in place (ADR 0175)", () => {
  it('a fresh qrtz has echo on and offers the download without touching the switch', async () => {
    const v = await mount(async () => none);
    expect(v.q('talk')).not.toBeNull();
    expect(v.q('echo-switch')?.getAttribute('aria-checked')).toBe('true');
    const offer = v.q('echo-offer');
    expect(offer).not.toBeNull();
    // The same consent text as the switch's dialog, all lowercase.
    expect(offer!.textContent).toContain(
      'echo needs its speech model, parakeet (628 MB). it downloads once from hugging face',
    );
    expect(v.q('echo-download')?.textContent).toBe('download parakeet');
    await act(async () => v.q('echo-download')!.click());
    await v.settle();
    expect(
      v.calls.filter((c) => c.channel === 'speech:download').map((c) => c.req),
    ).toEqual([{ extensions: ['echo'] }]);
    // Downloading now: the offer goes away (the notice shows the bar), and nothing touched the config.
    expect(v.q('echo-offer')).toBeNull();
    expect(v.calls.some((c) => c.channel === 'config:set')).toBe(false);
    v.unmount();
  });

  it('offers nothing while a download runs, and nothing once parakeet is installed', async () => {
    let s = downloading;
    const v = await mount(async () => s);
    expect(v.q('talk')).not.toBeNull();
    expect(v.q('echo-offer')).toBeNull();
    // The download finishes: the view reads echo's status again and still offers nothing.
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
