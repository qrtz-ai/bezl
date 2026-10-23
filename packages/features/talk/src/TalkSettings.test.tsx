// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act, useState, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import type { QrtzClient } from '@qrtz/core';
import { QrtzClientProvider, SubAppRouteProvider } from '@qrtz/core/react';
import { ipcSamples } from '@qrtz/core/ipc/samples';
import { ThemeProvider } from '@qrtz/ui';
import TalkView from './TalkView';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

/** The shell's route for this view: the sidebar ⋯ → opts puts it at `settings`. */
let go: (path: string) => void = () => {};
function Routed({ children }: { children: ReactNode }) {
  const [path, setPath] = useState('');
  go = setPath;
  return (
    <SubAppRouteProvider route={{ active: true, path, navigate: setPath }}>
      {children}
    </SubAppRouteProvider>
  );
}

describe("echo's own settings (ADR 0146 M1.4)", () => {
  it('open from the sidebar ⋯ (the settings route), write only the talk section through module:set-settings, and close', async () => {
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
            <Routed>
              <TalkView />
            </Routed>
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
    await act(async () => go('settings'));
    await settle();
    expect(q('talk-settings')?.textContent).toContain('echo opts');
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

describe("echo's dev opts (ADR 0221, temporary pill tuning)", () => {
  it('stay collapsed until opened, write the whole talk.pillMotion on commit, and reset to the snappy defaults', async () => {
    const { PillDevOpts } = await import('./PillDevOpts');
    const { PILL_MOTION_DEFAULTS } = await import('@qrtz/core');
    const commits: unknown[] = [];
    const el = document.createElement('div');
    document.body.appendChild(el);
    const root = createRoot(el);
    await act(async () =>
      root.render(
        <ThemeProvider mode="dark">
          <PillDevOpts
            value={{ expandBounce: 0.9, collapseDuration: 0.5 }}
            onCommit={(m) => commits.push(m)}
          />
        </ThemeProvider>,
      ),
    );
    const q = (id: string) =>
      el.querySelector<HTMLInputElement>(`[data-testid="${id}"]`);
    expect(q('talk-motion-expandBounce')).toBeNull();
    await act(async () => q('talk-dev-opts-toggle')!.click());
    // Stored values are clamped to the slider's range; missing ones take the default.
    expect(q('talk-motion-expandBounce')!.value).toBe('0.35');
    expect(q('talk-motion-collapseDuration')!.value).toBe('0.5');
    expect(q('talk-motion-expandDuration')!.value).toBe('0.3');
    expect(el.textContent).not.toMatch(/settings/i);
    await act(async () => q('talk-motion-reset')!.click());
    expect(commits).toEqual([PILL_MOTION_DEFAULTS]);
    expect(PILL_MOTION_DEFAULTS).toMatchObject({
      expandDuration: 0.3,
      expandBounce: 0.15,
      collapseDuration: 0.3,
      collapseBounce: 0,
    });
    root.unmount();
  });
});
