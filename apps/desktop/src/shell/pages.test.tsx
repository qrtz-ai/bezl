// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { createMemoryRouter, RouterProvider } from 'react-router';
import { createTauriTransport } from '@qrtz/core';
import { createMockBridge } from '../mock/backend';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

// The real sub apps behind the mocked Tauri backend, as `bun run dev:mock` runs them.
window.qrtzTransport = createTauriTransport(createMockBridge());
const { routes } = await import('./App');
const { registry } = await import('./subapps');

/** Every routed page: each sidebar app, each module's own settings, and the qrtz page (its list, an app's modal
 * over it, its settings). The qrtz page's parts never route by their own ids. */
const ROUTES: [host: string, path: string][] = [
  ...registry
    .list()
    .map((a) => a.manifest.id)
    .filter((id) => id !== 'marketplace' && id !== 'settings')
    .map((id) => [id, `/${id}`] as [string, string]),
  ['talk', '/talk/settings'],
  ['note', '/note/settings'],
  ['ctrl', '/ctrl/settings'],
  ['snap', '/snap/settings'],
  ['marketplace', '/qrtz'],
  ['marketplace', '/qrtz/item/echo'],
  ['settings', '/qrtz/settings'],
];

describe('page layout', () => {
  it.each(ROUTES)(
    '%s at %s renders inside the one shared page container',
    async (app, path) => {
      const router = createMemoryRouter(routes, { initialEntries: [path] });
      const el = document.createElement('div');
      document.body.appendChild(el);
      const root = createRoot(el);
      await act(async () => root.render(<RouterProvider router={router} />));
      const host = () =>
        el.querySelector<HTMLElement>(`[data-subapp="${app}"]`);
      for (let i = 0; i < 50 && !host()?.querySelector('[data-page]'); i++)
        await act(async () => {
          await new Promise((r) => setTimeout(r, 20));
        });
      const pages = host()!.querySelectorAll('[data-page]');
      // Exactly one page container per view, with no title or header row; nothing else sets page padding.
      expect(pages).toHaveLength(1);
      const page = pages[0] as HTMLElement;
      expect(page.querySelector('[data-page-header]')).toBeNull();
      expect(page.querySelector('h1')).toBeNull();
      expect(page.style.maxWidth).toBe('944px');
      expect(page.style.padding).toBe('40px 32px 48px');
      expect(page.style.overflowX).toBe('hidden');
      // The main area scrolls vertically only and never bounces.
      const main = el.querySelector<HTMLElement>('[data-scroll-root]')!;
      expect(main.style.overflowX).toBe('hidden');
      expect(main.style.overscrollBehavior).toBe('none');
      act(() => root.unmount());
      el.remove();
    },
  );
});
