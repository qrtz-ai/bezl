// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import type { DashView as View, QrtzClient } from '@qrtz/core';
import { ipcSamples } from '@qrtz/core/ipc/samples';
import { QrtzClientProvider } from '@qrtz/core/react';
import { ThemeProvider } from '@qrtz/ui';
import DashView from './DashView';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

function fake(connected: boolean, connectOk = true) {
  const calls: [string, unknown][] = [];
  let isConnected = connected;
  const client = {
    subApp: 'dash',
    invoke: async (channel: string, p: unknown) => {
      calls.push([channel, p]);
      if (channel === 'dash:view')
        return {
          ...ipcSamples['dash:view'].response,
          connected: isConnected,
        } satisfies View;
      if (channel === 'dash:connect') {
        if (!connectOk) throw new Error('E_FORBIDDEN');
        isConnected = true;
        return { connected: true, origin: 'https://dash.qrtz.ai' };
      }
      if (channel === 'dash:disconnect') isConnected = false;
      return { ok: true };
    },
    on: () => () => {},
  } as unknown as QrtzClient;
  return { client, calls };
}

let root: Root | null = null;
let el: HTMLElement;
afterEach(() => {
  act(() => root?.unmount());
  root = null;
  el?.remove();
});
const q = (id: string) => el.querySelector(`[data-testid="${id}"]`);
const flush = () =>
  act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
async function render(client: QrtzClient) {
  el = document.createElement('div');
  document.body.append(el);
  root = createRoot(el);
  await act(async () =>
    root!.render(
      <ThemeProvider>
        <QrtzClientProvider client={client}>
          <DashView />
        </QrtzClientProvider>
      </ThemeProvider>,
    ),
  );
  await flush();
}
const type = async (input: HTMLInputElement, value: string) =>
  act(async () => {
    const set = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      'value',
    )!.set!;
    set.call(input, value);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });

describe('dash server (ADR 0231)', () => {
  it('connects with a setup token, clears it, and shows the server', async () => {
    const { client, calls } = fake(false);
    await render(client);
    expect(q('dash-connected')).toBeNull();
    const input = q('dash-setup-token') as HTMLInputElement;
    expect(input.type).toBe('password');
    await type(input, 'the-setup-token');
    await act(async () => (q('dash-connect') as HTMLButtonElement).click());
    await flush();
    expect(calls).toContainEqual([
      'dash:connect',
      { setupToken: 'the-setup-token' },
    ]);
    expect(q('dash-connected')?.textContent).toContain('dash.qrtz.ai');
    expect(el.textContent).not.toContain('the-setup-token');
  });

  it('says so when the token is refused, all lowercase', async () => {
    const { client } = fake(false, false);
    await render(client);
    await type(q('dash-setup-token') as HTMLInputElement, 'bad');
    await act(async () => (q('dash-connect') as HTMLButtonElement).click());
    await flush();
    expect(el.textContent).toContain("couldn't connect");
    expect(el.textContent).toBe(el.textContent?.toLowerCase());
  });
});
