// @vitest-environment jsdom
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import type { AppUpdateStatus, QrtzClient } from '@qrtz/core';
import { appUpdateDot } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { ThemeProvider } from '@qrtz/ui';
import AppUpdateRow, {
  appUpdateLine,
  failedUpdate,
  RELEASES_URL,
} from './AppUpdateRow';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

// The update row is what a release build shows; vitest runs as DEV.
beforeAll(() => vi.stubEnv('DEV', false));
afterAll(() => vi.unstubAllEnvs());

const base: AppUpdateStatus = {
  current: '0.2.0',
  state: 'idle',
  latest: null,
  notes: null,
  sizeBytes: null,
  doneBytes: null,
  error: null,
  checkedAtMs: null,
};
const st = (p: Partial<AppUpdateStatus>): AppUpdateStatus => ({
  ...base,
  ...p,
});

async function mount(
  start: AppUpdateStatus,
  answers: Record<string, unknown> = {},
) {
  const calls: string[] = [];
  let push: ((s: AppUpdateStatus) => void) | null = null;
  const client = {
    subApp: 'settings',
    invoke: async (channel: string) => {
      calls.push(channel);
      if (channel === 'app-update:status') return start;
      return answers[channel];
    },
    on: (event: string, cb: (s: AppUpdateStatus) => void) => {
      if (event === 'app-update:state') push = cb;
      return () => {};
    },
  } as unknown as QrtzClient;
  const el = document.createElement('div');
  document.body.appendChild(el);
  const root = createRoot(el);
  await act(async () =>
    root.render(
      <ThemeProvider mode="light">
        <QrtzClientProvider client={client}>
          <AppUpdateRow />
        </QrtzClientProvider>
      </ThemeProvider>,
    ),
  );
  await act(async () => {
    await new Promise((r) => setTimeout(r, 5));
  });
  const q = (id: string) =>
    el.querySelector(`[data-testid="${id}"]`) as HTMLElement | null;
  const emit = async (s: AppUpdateStatus) =>
    act(async () => {
      push?.(s);
      await new Promise((r) => setTimeout(r, 1));
    });
  const click = async (id: string) =>
    act(async () => {
      q(id)?.click();
      await new Promise((r) => setTimeout(r, 5));
    });
  return { el, q, calls, emit, click, done: () => root.unmount() };
}

describe('settings → qrtz: its own update (ADR 0181)', () => {
  it('shows the version and checks only when pressed', async () => {
    const m = await mount(base, {
      'app-update:check': st({ state: 'up-to-date', checkedAtMs: 1 }),
    });
    expect(m.el.textContent).toContain('qrtz 0.2.0');
    expect(m.calls).toEqual(['app-update:status']);
    expect(m.q('app-update-download')).toBeNull();
    expect(m.q('app-update-restart')).toBeNull();
    await m.click('update-check');
    expect(m.calls).toContain('app-update:check');
    expect(m.q('update-result')?.textContent).toBe('qrtz 0.2.0 is up to date');
    m.done();
  });

  it('available → update with progress → restart to update', async () => {
    const avail = st({
      state: 'available',
      latest: '0.2.1',
      sizeBytes: 31_400_000,
      notes: 'Faster dictation',
    });
    const m = await mount(avail, {
      'app-update:download': st({
        state: 'downloading',
        latest: '0.2.1',
        sizeBytes: 31_400_000,
        doneBytes: 0,
      }),
      'app-update:restart': { restarting: true },
    });
    expect(m.q('update-result')?.textContent).toBe(
      'qrtz 0.2.1 is available (31.4 MB)',
    );
    expect(m.q('app-update-notes')?.textContent).toBe('faster dictation');
    await m.click('app-update-download');
    expect(m.calls).toContain('app-update:download');
    await m.emit(
      st({
        state: 'downloading',
        latest: '0.2.1',
        sizeBytes: 31_400_000,
        doneBytes: 15_700_000,
      }),
    );
    expect(m.q('app-update-progress')?.getAttribute('aria-valuenow')).toBe(
      '50',
    );
    expect(m.q('app-update-download')).toBeNull();
    expect((m.q('update-check') as HTMLButtonElement).disabled).toBe(true);
    await m.emit(st({ state: 'installing', latest: '0.2.1' }));
    expect(m.q('app-update-progress')).toBeNull();
    await m.emit(st({ state: 'ready', latest: '0.2.1' }));
    expect(m.q('update-result')?.textContent).toBe(
      'qrtz 0.2.1 is ready: restart to update',
    );
    expect(m.q('app-update-restart')?.textContent).toBe('restart to update');
    await m.click('app-update-restart');
    expect(m.calls).toContain('app-update:restart');
    expect(m.q('app-update-error')).toBeNull();
    m.done();
  });

  it('a failure keeps "update" when there is a release to retry; not set up disables the check', async () => {
    const m = await mount(
      st({ state: 'failed', latest: '0.2.1', error: 'signature mismatch' }),
    );
    expect(m.q('update-result')?.textContent).toContain("couldn't update qrtz");
    expect(m.q('update-result')?.textContent).toContain('signature mismatch');
    expect(m.q('app-update-download')).not.toBeNull();
    m.done();
    const n = await mount(st({ state: 'not-set-up' }));
    expect((n.q('update-check') as HTMLButtonElement).disabled).toBe(true);
    expect(n.q('update-result')?.textContent).toBe(
      "updates aren't set up in this build yet",
    );
    n.done();
  });

  it('every state has lowercase text, and the dot shows only for available or ready', () => {
    for (const state of [
      'not-set-up',
      'idle',
      'checking',
      'up-to-date',
      'available',
      'downloading',
      'installing',
      'ready',
      'failed',
    ] as const) {
      const s = st({ state, latest: '0.2.1', sizeBytes: 1e6, doneBytes: 1e5 });
      const line = appUpdateLine(s);
      // Sizes keep their unit ("MB", as backup says it); everything else is lowercase.
      expect(line.replace(/MB/g, ''), state).toBe(
        line.replace(/MB/g, '').toLowerCase(),
      );
      expect(appUpdateDot(s), state).toBe(
        state === 'available' || state === 'ready',
      );
    }
  });

  it('QRT-313: a blocked redirect says what happened, links the releases page, and keeps the host under details', async () => {
    const blocked = st({
      state: 'failed',
      latest: '0.2.5',
      error: 'update_redirect_blocked: evil.example.com',
      errorCode: 'update_redirect_blocked',
      errorDetails: { host: 'evil.example.com', scheme: 'https' },
    });
    const e = failedUpdate(blocked);
    expect(e.title).toBe("couldn't download the update");
    expect(e.next).toBe(
      'The download was sent to an unexpected server, so qrtz stopped it. Your current version is fine. Try again later, or get the update from the qrtz releases page.',
    );
    expect(e.detail).toBe('blocked host: evil.example.com');
    expect(e.next).not.toContain('evil.example.com');
    expect(
      failedUpdate({
        ...blocked,
        errorDetails: { host: 'evil.example.com', scheme: 'http' },
      }).detail,
    ).toBe('blocked host: evil.example.com (http)');
    expect(failedUpdate({ ...blocked, errorDetails: null }).detail).toBe(
      'update_redirect_blocked: evil.example.com',
    );

    const m = await mount(blocked);
    const r = m.q('update-result');
    expect(r?.getAttribute('role')).toBe('alert');
    expect(r?.textContent).toContain("couldn't download the update");
    expect(r?.textContent).toContain('Your current version is fine.');
    const link = m.q('app-update-releases-link') as HTMLAnchorElement;
    expect(link.textContent).toBe('qrtz releases page');
    expect(link.getAttribute('href')).toBe(RELEASES_URL);
    expect(link.getAttribute('rel')).toBe('noopener noreferrer');
    expect(m.q('update-result-details')?.textContent).toContain(
      'blocked host: evil.example.com',
    );
    expect(m.q('app-update-download')).not.toBeNull();
    m.done();
  });
});

describe('a dev build', () => {
  it('says dev and the commit, and never asks the host about updates', async () => {
    vi.stubEnv('DEV', true);
    vi.stubEnv('VITE_QRTZ_COMMIT', 'abc1234');
    const { q, calls } = await mount(st({ state: 'up-to-date' }));
    expect(q('app-update-dev')?.textContent).toContain('qrtz dev · abc1234');
    expect(q('update-check')).toBeNull();
    expect(calls).toEqual([]);
    vi.stubEnv('DEV', false);
  });
});
