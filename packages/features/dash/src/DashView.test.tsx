// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import {
  AppError,
  DashBlocks,
  isHttpsUrl,
  DashView as DashViewSchema,
  type DashView as View,
  type QrtzClient,
} from '@qrtz/core';
import { ipcSamples } from '@qrtz/core/ipc/samples';
import { QrtzClientProvider } from '@qrtz/core/react';
import { primaryButtons, ThemeProvider } from '@qrtz/ui';
import DashView from './DashView';
import { openLink } from './blocks';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const view = (): View => ({
  ...ipcSamples['dash:view'].response,
  agents: [
    {
      id: 'ag_1',
      name: 'grok bot',
      description: '',
      fingerprint: '7F3A-91C2',
      status: 'approved',
      heartbeat: 'running',
      lastSeen: Date.now() - 11 * 60_000,
      stale: true,
      flagged: false,
      createdAt: 0,
      widgets: [
        {
          key: 'state-farm',
          type: 'status',
          title: 'state farm',
          state: { status: 'blocked', text: 'waiting on user id' },
          events: [],
          updatedAt: 0,
        },
        {
          key: 'evil',
          type: 'status',
          title: 'evil',
          state: { status: 'needs_you', action: 'click' },
          events: [],
          updatedAt: 0,
        },
      ],
    },
    {
      id: 'ag_2',
      name: 'new one',
      description: '',
      fingerprint: '1C0D-44AE',
      status: 'pending',
      heartbeat: null,
      lastSeen: 0,
      stale: false,
      flagged: false,
      createdAt: 0,
      widgets: [],
    },
  ],
});

function fake(over: Partial<View> = {}, copyError?: unknown) {
  const calls: [string, unknown][] = [];
  const client = {
    subApp: 'dash',
    invoke: async (channel: string, p: unknown) => {
      calls.push([channel, p]);
      if (channel === 'dash:view') return { ...view(), ...over };
      if (channel === 'dash:copy-prompt') {
        if (copyError) throw copyError;
        return { copied: true };
      }
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

describe('dash view (ADR 0228)', () => {
  it('shows agents read-only, with stale, pending review and no action ui', async () => {
    const { client, calls } = fake();
    await render(client);
    expect(primaryButtons(document.body)).toBeLessThanOrEqual(1);
    expect(q('dash-agent-ag_1')?.textContent).toContain('7F3A-91C2');
    expect(q('dash-stale')).not.toBeNull();
    expect(q('dash-widget-state-farm')?.textContent).toContain(
      'waiting on user id',
    );
    // a payload that doesn't fit its type is not shown
    expect(q('dash-widget-evil')?.textContent).toContain('nothing yet');
    expect(el.textContent).not.toMatch(/needs you|settings/);
    expect(el.textContent).toBe(
      el.textContent
        ?.toLowerCase()
        .replace(/7f3a-91c2|1c0d-44ae/g, (m) => m.toUpperCase()),
    );
    const pending = q('dash-pending-ag_2');
    const approve = [...(pending?.querySelectorAll('button') ?? [])].find(
      (b) => b.textContent === 'approve',
    );
    await act(async () => approve?.click());
    expect(calls).toContainEqual([
      'dash:review-agent',
      { agentId: 'ag_2', approve: true },
    ]);
  });

  it('deletes widget, agent and dashboard', async () => {
    const { client, calls } = fake();
    await render(client);
    const del = (label: string) =>
      el.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`);
    await act(async () => del('delete state farm')?.click());
    await act(async () => del('delete grok bot')?.click());
    await act(async () => (q('dash-delete') as HTMLButtonElement).click());
    await act(async () =>
      (q('dash-delete-confirm') as HTMLButtonElement).click(),
    );
    const dels = calls.filter(([c]) => c === 'dash:delete').map(([, p]) => p);
    expect(dels).toEqual([
      { scope: 'widget', agentId: 'ag_1', key: 'state-farm' },
      { scope: 'agent', agentId: 'ag_1' },
      { scope: 'dashboard' },
    ]);
  });
});

describe('dash copy prompt (QRT-298)', () => {
  const btn = () => q('dash-copy-prompt') as HTMLButtonElement | null;
  it('never issues a code or copies on render', async () => {
    const { client, calls } = fake();
    await render(client);
    expect(btn()).not.toBeNull();
    expect(calls.map(([c]) => c)).not.toContain('dash:copy-prompt');
    expect(calls.map(([c]) => c)).not.toContain('dash:issue-code');
    expect(el.textContent).not.toMatch(/ENR-|v1\/readme/);
  });
  it('is disabled and says not connected with no origin', async () => {
    const { client, calls } = fake({ origin: '' });
    await render(client);
    expect(btn()?.disabled).toBe(true);
    expect(q('dash-not-connected')?.textContent).toBe(
      "agents can't enroll until a server is connected",
    );
    await act(async () => btn()?.click());
    expect(calls.map(([c]) => c)).not.toContain('dash:copy-prompt');
  });
  it('a not_connected refusal disables enrolling the same way', async () => {
    const { client, calls } = fake(
      { origin: 'http://127.0.0.1:8799', connected: true },
      new AppError('E_UNAVAILABLE', 'not_connected'),
    );
    await render(client);
    await act(async () => btn()?.click());
    await flush();
    expect(q('dash-copied')).toBeNull();
    expect(btn()?.disabled).toBe(true);
    expect(q('dash-not-connected')?.textContent).toBe(
      "agents can't enroll until a server is connected",
    );
    await act(async () => btn()?.click());
    expect(calls.filter(([c]) => c === 'dash:copy-prompt')).toHaveLength(1);
  });
  it('other copy errors keep the button enabled', async () => {
    const { client } = fake(
      { origin: 'https://dash.qrtz.ai', connected: true },
      new AppError('E_INTERNAL', 'boom'),
    );
    await render(client);
    await act(async () => btn()?.click());
    await flush();
    expect(btn()?.disabled).toBe(false);
    expect(q('dash-not-connected')).toBeNull();
  });
  for (const origin of ['https://dash.qrtz.ai', 'http://127.0.0.1:8799'])
    it(`copies on click and shows copied (${origin})`, async () => {
      const { client, calls } = fake({ origin, connected: true });
      await render(client);
      expect(btn()?.disabled).toBe(false);
      expect(q('dash-copied')).toBeNull();
      await act(async () => btn()?.click());
      await flush();
      expect(calls.filter(([c]) => c === 'dash:copy-prompt')).toHaveLength(1);
      expect(q('dash-copied')?.textContent).toBe('copied');
      expect(el.textContent).not.toMatch(/ENR-/);
    });
});

describe('dash blocks', () => {
  it('match the rust rules on the shared samples', () => {
    expect(
      DashViewSchema.safeParse(ipcSamples['dash:view'].response).success,
    ).toBe(true);
    expect(
      DashBlocks.status.safeParse({ status: 'done', extra: 1 }).success,
    ).toBe(false);
    expect(
      DashBlocks.list.safeParse({
        items: [{ text: 'x', url: 'javascript:alert(1)' }],
      }).success,
    ).toBe(false);
    expect(
      DashBlocks.list.safeParse({
        items: [{ text: 'x', url: 'https://qrtz.ai/a' }],
      }).success,
    ).toBe(true);
    expect(DashBlocks.markdown.safeParse({ text: '<b>x</b>' }).success).toBe(
      false,
    );
    expect(
      DashBlocks.markdown.safeParse({ text: '![i](https://a.co/i.png)' })
        .success,
    ).toBe(false);
    expect(DashBlocks.progress.safeParse({ done: 6, total: 5 }).success).toBe(
      false,
    );
  });

  it('asks once per domain, warns on punycode, and opens through the host only', async () => {
    const opened: string[] = [];
    const open = async (u: string) => (opened.push(u), true);
    const asked: string[] = [];
    const ask = (m: string) => (asked.push(m), true);
    window.open = (() => {
      throw new Error('the webview must never open links');
    }) as typeof window.open;
    expect(await openLink('http://a.co', open, ask)).toBe(false);
    expect(await openLink('https://a.co:x/', open, ask)).toBe(false);
    expect(await openLink('https://b.co/1', open, ask)).toBe(true);
    expect(await openLink('https://b.co/2', open, ask)).toBe(true);
    expect(await openLink('https://xn--pple-43d.com/', open, ask)).toBe(true);
    expect(asked).toHaveLength(2);
    expect(asked[1]).toContain('xn--');
    expect(opened).toEqual([
      'https://b.co/1',
      'https://b.co/2',
      'https://xn--pple-43d.com/',
    ]);
  });

  it('matches the rust url rules', () => {
    for (const u of ['https://a.com:8443/x', 'https://a-b.c.io?q=1#f'])
      expect(isHttpsUrl(u)).toBe(true);
    for (const u of [
      'https://a.com:',
      'https://a.com:99999',
      'https://a.com:1:2',
      'https://a..com',
      'https://user@a.com',
    ])
      expect(isHttpsUrl(u)).toBe(false);
  });
});
