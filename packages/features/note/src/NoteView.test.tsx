// @vitest-environment jsdom
// nano's opts take over the whole page, like echo's and ctrl's (ADR 0146 M1.4; Jared, 0.2.4): the meeting list,
// the hud and the record prompt are gone while they're open and back on close.
import { describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import type { QrtzClient } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { ipcSamples } from '@qrtz/core/ipc/samples';
import { primaryButtons, ThemeProvider } from '@qrtz/ui';
import NoteView from './NoteView';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

async function mount(calls: { channel: string; req: unknown }[] = []) {
  const client = {
    subApp: 'note',
    invoke: async (channel: string, req: unknown) => {
      calls.push({ channel, req });
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
  expect(primaryButtons(el)).toBeLessThanOrEqual(1);
  const q = (id: string) =>
    el.querySelector<HTMLElement>(`[data-testid="${id}"]`);
  return { el, q, settle, unmount: () => root.unmount() };
}

describe("nano's opts", () => {
  it('open in a modal over the view and close on the button, esc and the scrim', async () => {
    const v = await mount();
    expect(v.q('note-app')).not.toBeNull();
    await act(async () => v.q('note-settings-open')!.click());
    await v.settle();
    const dialog = v.q('note-settings')!;
    expect(dialog).not.toBeNull();
    expect(dialog.getAttribute('role')).toBe('dialog');
    expect(dialog.getAttribute('aria-modal')).toBe('true');
    expect(dialog.contains(document.activeElement)).toBe(true);
    // The view stays underneath.
    expect(v.q('note-app')).not.toBeNull();
    expect(v.el.querySelector('[aria-label="meetings"]')).not.toBeNull();
    await act(async () => v.q('note-settings-close')!.click());
    await v.settle();
    expect(v.q('note-settings')).toBeNull();

    await act(async () => v.q('note-settings-open')!.click());
    await v.settle();
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    });
    await v.settle();
    expect(v.q('note-settings')).toBeNull();

    await act(async () => v.q('note-settings-open')!.click());
    await v.settle();
    const scrim = v.q('note-settings-scrim')!;
    await act(async () => {
      scrim.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    });
    await v.settle();
    expect(v.q('note-settings')).toBeNull();
    expect(v.q('note-app')).not.toBeNull();
    v.unmount();
  });
});

describe("nano's speech notice (QRT-277)", () => {
  it("asks about only nano's models", async () => {
    const calls: { channel: string; req: unknown }[] = [];
    const v = await mount(calls);
    const status = calls.filter((c) => c.channel === 'speech:status');
    expect(status.length).toBeGreaterThan(0);
    for (const c of status) expect(c.req).toEqual({ extensions: ['nano'] });
    v.unmount();
  });
});
