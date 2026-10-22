// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import type { QrtzClient } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { primaryButtons, ThemeProvider } from '@qrtz/ui';
import SnapView from './SnapView';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

/** snap:status answers from `trusted`, as qrtz-app's live read does. */
function fake(state: { trusted: boolean; im?: boolean }) {
  const calls: string[] = [];
  const client = {
    subApp: 'snap',
    invoke: async (channel: string, payload?: unknown) => {
      calls.push(channel);
      if (channel === 'snap:status')
        return {
          running: true,
          axTrusted: state.trusted,
          registered: { leftHalf: 'alt+cmd+left' },
          refused: {},
          inputMonitoring: !!state.im,
          summonActive: !!state.im,
          apps: [],
        };
      if (channel === 'module:get-settings')
        return { snap: { shortcuts: {}, summon: { enabled: true } } };
      if (channel === 'module:set-settings') return {};
      if (channel === 'snap:request-access') {
        calls.push(`snap:request-access:${JSON.stringify(payload ?? null)}`);
        return { opened: true, axTrusted: state.trusted };
      }
      return undefined;
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
const wait = (ms: number) =>
  act(async () => {
    await new Promise((r) => setTimeout(r, ms));
  });
const q = (id: string) => el.querySelector(`[data-testid="${id}"]`);

async function render(client: QrtzClient) {
  el = document.createElement('div');
  document.body.appendChild(el);
  root = createRoot(el);
  await act(async () =>
    root!.render(
      <ThemeProvider mode="light">
        <QrtzClientProvider client={client}>
          <SnapView />
        </QrtzClientProvider>
      </ThemeProvider>,
    ),
  );
  await wait(10);
}

describe('snap view: accessibility from inside qrtz', () => {
  it('shows the prompt with one button and one line, asks qrtz on the press, and drops the prompt once qrtz is switched on', async () => {
    const state = { trusted: false };
    const f = fake(state);
    await render(f.client);
    expect(primaryButtons(document.body)).toBeLessThanOrEqual(1);
    const prompt = q('snap-no-ax');
    expect(prompt?.textContent).toContain('snap needs accessibility access');
    expect(prompt?.textContent).toContain('switch on qrtz');
    expect(prompt?.textContent).not.toMatch(/[A-Z]/);
    expect(prompt?.textContent).not.toContain('privacy & security');
    const open = q('snap-no-ax-open') as HTMLButtonElement;
    expect(open.textContent).toContain('open accessibility settings');
    await act(async () => open.click());
    await wait(10);
    expect(f.calls).toContain('snap:request-access');
    expect(q('snap-no-ax-waiting')).not.toBeNull();
    // The user switches qrtz on in system settings: the 1s re-read sees it, no relaunch.
    state.trusted = true;
    await wait(1100);
    expect(q('snap-no-ax')).toBeNull();
  });

  it('never shows the prompt while qrtz already holds the grant (the stale-banner bug)', async () => {
    const f = fake({ trusted: true });
    await render(f.client);
    expect(q('snap-no-ax')).toBeNull();
    const before = f.calls.filter((c) => c === 'snap:status').length;
    await wait(1100);
    expect(f.calls.filter((c) => c === 'snap:status').length).toBe(before);
  });
});

describe('snap view: shortcuts and summon are the main page', () => {
  it('shows summon with its permission state and every shortcut editor, not in opts', async () => {
    const f = fake({ trusted: true });
    await render(f.client);
    expect(q('snap-settings-summon')).not.toBeNull();
    expect(q('snap-summon-no-im')?.textContent).toContain(
      'summon needs input monitoring',
    );
    expect(q('snap-settings-shortcuts')).not.toBeNull();
    expect(q('snap-shortcut-leftHalf')).not.toBeNull();
    expect(q('snap-clear-leftHalf')).not.toBeNull();
    expect(q('snap-default-leftHalf')).not.toBeNull();
    expect(q('snap-restore-all')).not.toBeNull();
    expect(q('snap-group-0')).not.toBeNull();
  });
});

describe('snap view: summon asks for input monitoring from inside qrtz', () => {
  it("asks qrtz-app for it on the press and drops the prompt once it's on", async () => {
    const state = { trusted: true, im: false };
    const f = fake(state);
    await render(f.client);
    expect(primaryButtons(document.body)).toBeLessThanOrEqual(1);
    const prompt = q('snap-summon-no-im');
    expect(prompt?.textContent).toContain('summon needs input monitoring');
    const open = q('snap-summon-no-im-open') as HTMLButtonElement;
    expect(open.textContent).toContain('open input monitoring settings');
    await act(async () => open.click());
    await wait(10);
    expect(f.calls).toContain(
      'snap:request-access:{"permission":"input-monitoring"}',
    );
    expect(q('snap-summon-no-im-waiting')).not.toBeNull();
    state.im = true;
    await wait(1100);
    expect(q('snap-summon-no-im')).toBeNull();
    expect(q('snap-summon-ready')).not.toBeNull();
  });

  it('waits for accessibility first: one grant button at a time', async () => {
    const f = fake({ trusted: false, im: false });
    await render(f.client);
    expect(q('snap-no-ax')).not.toBeNull();
    expect(q('snap-summon-no-im')).toBeNull();
  });
});
