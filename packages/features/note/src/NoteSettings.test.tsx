// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import type { QrtzClient } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { ThemeProvider } from '@qrtz/ui';
import NoteSettings from './NoteSettings';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

describe("nano's own settings (ADR 0146 M1.4)", () => {
  it('show recording, speakers and summaries, and write each through module:set-settings', async () => {
    const sets: unknown[] = [];
    const client = {
      subApp: 'note',
      invoke: async (channel: string, req: unknown) => {
        if (channel === 'module:get-settings')
          return {
            note: { autoRecord: false, retainAudioDays: 0 },
            speech: { speakerMode: 'fast' },
            privacy: { excludedApps: ['slack'], pauseHotkey: 'X' },
          };
        if (channel === 'module:set-settings') {
          sets.push(req);
          return {};
        }
        throw new Error(`unexpected ${channel}`);
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
            <NoteSettings />
          </QrtzClientProvider>
        </ThemeProvider>,
      ),
    );
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
    const q = <T extends HTMLElement = HTMLElement>(id: string) =>
      el.querySelector<T>(`[data-testid="${id}"]`);
    expect(q('note-settings-recording')).not.toBeNull();
    expect(q('note-settings-speakers')).not.toBeNull();
    expect(q('note-settings-summaries')).not.toBeNull();
    expect(q<HTMLInputElement>('excluded-apps')?.value).toBe('slack');
    expect(q('talk-show-pill')).toBeNull();
    await act(async () => q('auto-record')!.click());
    const mode = q<HTMLSelectElement>('speaker-mode')!;
    await act(async () => {
      mode.value = 'accurate';
      mode.dispatchEvent(new Event('change', { bubbles: true }));
    });
    expect(sets).toEqual([
      { patch: { note: { autoRecord: true } } },
      { patch: { speech: { speakerMode: 'accurate' } } },
    ]);
    root.unmount();
  });
});
