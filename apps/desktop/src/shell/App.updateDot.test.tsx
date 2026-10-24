// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { createMemoryRouter, RouterProvider } from 'react-router';
import { defineSubApp, type QrtzTransport } from '@qrtz/core';
import { ipcSamples } from '@qrtz/core/ipc/samples';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

vi.mock('./subapps', async () => {
  const { createRegistry } = await import('@qrtz/core');
  const app = (id: string) =>
    defineSubApp({
      manifest: { id, name: id, icon: id },
      load: async () => ({ default: () => <div data-testid={`view-${id}`} /> }),
    });
  return { registry: createRegistry([app('note'), app('settings')]) };
});

const idle = ipcSamples['app-update:status'].response;
let listener: ((p: unknown) => void) | null = null;
const transport: QrtzTransport = {
  invoke: async (e) => ({
    ok: true,
    id: e.id,
    data:
      e.channel === 'app-update:status'
        ? idle
        : // ADR 0184: past the first-run permissions onboarding, so the sidebar shows.
          e.channel === 'config:get'
          ? { onboarding: { done: true } }
          : (ipcSamples as Record<string, { response: unknown }>)[e.channel]
              ?.response,
  }),
  on: (_subApp, event, cb) => {
    if (event === 'app-update:state') listener = cb;
    return () => {};
  },
};
window.qrtzTransport = transport;

const { routes } = await import('./App');

describe('the sidebar dot (ADR 0181)', () => {
  it('shows on the opts row only while an update is available or ready', async () => {
    const router = createMemoryRouter(routes, { initialEntries: ['/note'] });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const root = createRoot(el);
    await act(async () => root.render(<RouterProvider router={router} />));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
    const dot = () => el.querySelector('[data-testid="nav-opts-dot"]');
    expect(dot()).toBeNull();
    const push = async (state: string) =>
      act(async () => {
        listener?.({ ...idle, state, latest: '0.2.1' });
        await new Promise((r) => setTimeout(r, 1));
      });
    await push('available');
    expect(dot()?.getAttribute('aria-label')).toBe('bezl update ready');
    expect(el.querySelector('[data-testid="nav-note-dot"]')).toBeNull();
    await push('downloading');
    expect(dot()).toBeNull();
    await push('ready');
    expect(dot()).not.toBeNull();
    await push('up-to-date');
    expect(dot()).toBeNull();
    root.unmount();
  });
});
