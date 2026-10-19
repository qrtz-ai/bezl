// @vitest-environment jsdom
// nano's opts take over the whole page, like echo's and ctrl's (ADR 0146 M1.4; Jared, 0.2.4): the meeting list,
// the hud and the record prompt are gone while they're open and back on close.
import { describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import type { QrtzClient } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { ipcSamples } from '@qrtz/core/ipc/samples';
import { ThemeProvider } from '@qrtz/ui';
import NoteView from './NoteView';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

async function mount() {
  const client = {
    subApp: 'note',
    invoke: async (channel: string) =>
      (ipcSamples as Record<string, { response: unknown }>)[channel]?.response,
    on: () => () => {},
  } as unknown as QrtzClient;
  const el = document.createElement('div');
  document.body.appendChild(el);
  const root = createRoot(el);
  await act(async () =>
    root.render(
      <ThemeProvider mode="light">
        <QrtzClientProvider client={client}>
          <NoteView />
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
  return { el, q, settle, unmount: () => root.unmount() };
}

describe("nano's opts", () => {
  it('take over the whole page and give it back on close', async () => {
    const v = await mount();
    expect(v.q('note-app')).not.toBeNull();
    await act(async () => v.q('note-settings-open')!.click());
    await v.settle();
    expect(v.q('note-settings')).not.toBeNull();
    expect(v.q('note-app')).toBeNull();
    expect(v.el.querySelector('[aria-label="meetings"]')).toBeNull();
    expect(v.q('note-record-manual')).toBeNull();
    expect(
      v.q('note-settings')!.closest('[data-testid="note-app"]'),
    ).toBeNull();
    await act(async () => v.q('note-settings-close')!.click());
    await v.settle();
    expect(v.q('note-settings')).toBeNull();
    expect(v.q('note-app')).not.toBeNull();
    expect(v.el.querySelector('[aria-label="meetings"]')).not.toBeNull();
    v.unmount();
  });
});
