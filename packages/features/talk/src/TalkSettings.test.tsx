// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import type { QrtzClient } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { ipcSamples } from '@qrtz/core/ipc/samples';
import { ThemeProvider } from '@qrtz/ui';
import TalkView from './TalkView';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

describe("echo's own settings (ADR 0146 M1.4)", () => {
  it('open from the gear in echo, write only the talk section through module:set-settings, and close', async () => {
    const calls: { channel: string; req: unknown }[] = [];
    let talk: Record<string, unknown> = { showPill: true, pushKey: 'fn' };
    const client = {
      subApp: 'talk',
      invoke: async (channel: string, req: unknown) => {
        calls.push({ channel, req });
        if (channel === 'module:get-settings') return { talk };
        if (channel === 'module:set-settings') {
          talk = {
            ...talk,
            ...(req as { patch: { talk: object } }).patch.talk,
          };
          return { talk };
        }
        return (ipcSamples as Record<string, { response: unknown }>)[channel]
          ?.response;
      },
      on: () => () => {},
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
    const q = (id: string) =>
      el.querySelector<HTMLElement>(`[data-testid="${id}"]`);
    const settle = () =>
      act(async () => {
        await new Promise((r) => setTimeout(r, 10));
      });
    await settle();
    expect(q('talk-settings')).toBeNull();
    expect(calls.some((c) => c.channel === 'module:get-settings')).toBe(false);
    await act(async () => q('talk-settings-open')!.click());
    await settle();
    expect(q('talk-settings')?.textContent).toContain('echo settings');
    expect(q('talk-show-pill')?.getAttribute('aria-checked')).toBe('true');
    // Nothing of nano's or the global settings here.
    expect(q('auto-record')).toBeNull();
    expect(q('modules-all')).toBeNull();
    await act(async () => q('talk-show-pill')!.click());
    await settle();
    expect(calls.find((c) => c.channel === 'module:set-settings')?.req).toEqual(
      { patch: { talk: { showPill: false } } },
    );
    expect(calls.some((c) => c.channel === 'config:set')).toBe(false);
    expect(q('talk-show-pill')?.getAttribute('aria-checked')).toBe('false');
    await act(async () => q('talk-settings-close')!.click());
    expect(q('talk-settings')).toBeNull();
    expect(q('talk')).not.toBeNull();
    root.unmount();
  });
});
