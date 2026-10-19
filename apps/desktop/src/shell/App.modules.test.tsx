// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { createMemoryRouter, RouterProvider } from 'react-router';
import { ConfigSchema, defineSubApp } from '@qrtz/core';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

// ADR 0146: nano switched off on the qrtz page.
vi.mock('./useModuleSwitches', () => ({
  useModuleSwitches: () =>
    ConfigSchema.parse({ modules: { nano: { enabled: false } } }),
}));

vi.mock('./subapps', async () => {
  const { createRegistry } = await import('@qrtz/core');
  const app = (id: string) =>
    defineSubApp({
      manifest: { id, name: id, icon: id },
      load: async () => ({
        default: () => <div data-testid={`view-${id}`} />,
      }),
    });
  return {
    registry: createRegistry([
      app('note'),
      app('talk'),
      app('marketplace'),
      app('settings'),
    ]),
  };
});

const { routes } = await import('./App');

describe('module switches (ADR 0146)', () => {
  it('a switched-off module has no tab, and its route falls back to the qrtz page', async () => {
    const router = createMemoryRouter(routes, {
      initialEntries: ['/note/meetings/m1'],
    });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const root = createRoot(el);
    await act(async () => root.render(<RouterProvider router={router} />));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
    const q = (id: string) => el.querySelector(`[data-testid="${id}"]`);
    expect(q('nav-note')).toBeNull();
    expect(q('nav-talk')).not.toBeNull();
    expect(q('nav-apps')).not.toBeNull();
    expect(q('view-note')).toBeNull();
    expect(router.state.location.pathname).toBe('/qrtz');
    expect(q('view-marketplace')).not.toBeNull();
    root.unmount();
  });
});
