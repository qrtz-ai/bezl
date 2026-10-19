// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import {
  DashBlocks,
  DashView as DashViewSchema,
  type DashView as View,
  type QrtzClient,
} from '@qrtz/core';
import { ipcSamples } from '@qrtz/core/ipc/samples';
import { QrtzClientProvider } from '@qrtz/core/react';
import { ThemeProvider } from '@qrtz/ui';
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

function fake() {
  const calls: [string, unknown][] = [];
  const client = {
    subApp: 'dash',
    invoke: async (channel: string, p: unknown) => {
      calls.push([channel, p]);
      if (channel === 'dash:view') return view();
      if (channel === 'dash:issue-code')
        return { code: 'ENR-ABCD-2345', expiresAt: 1 };
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

  it('issues a code and deletes widget, agent and dashboard', async () => {
    const { client, calls } = fake();
    await render(client);
    await act(async () => (q('dash-add-agent') as HTMLButtonElement).click());
    await flush();
    expect(q('dash-code')?.textContent).toContain('ENR-ABCD-2345');
    expect(q('dash-code')?.textContent).toContain(
      'https://dash.qrtz.ai/v1/readme',
    );
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

  it('asks once per domain before opening a link, and never opens non-https', () => {
    const opened: string[] = [];
    window.open = ((u: string) => {
      opened.push(u);
      return null;
    }) as typeof window.open;
    let asked = 0;
    const ask = () => (asked++, true);
    expect(openLink('http://a.co', ask)).toBe(false);
    expect(openLink('https://b.co/1', ask)).toBe(true);
    expect(openLink('https://b.co/2', ask)).toBe(true);
    expect(asked).toBe(1);
    expect(opened).toEqual(['https://b.co/1', 'https://b.co/2']);
  });
});
