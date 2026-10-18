// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';
import { act, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { createMemoryRouter, RouterProvider } from 'react-router';
import { defineSubApp, requestNavigate } from '@qrtz/core';
import { useSubAppRoute } from '@qrtz/core/react';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const mounts: Record<string, number> = {};
let releaseTalk: () => void = () => {};
const talkLoaded = new Promise<void>((r) => (releaseTalk = r));

function view(id: string) {
  return function View() {
    const route = useSubAppRoute();
    const [n, setN] = useState(0);
    useEffect(() => {
      mounts[id] = (mounts[id] ?? 0) + 1;
    }, []);
    return (
      <div data-testid={`view-${id}`}>
        <span data-testid={`path-${id}`}>{route.path}</span>
        <span data-testid={`active-${id}`}>{String(route.active)}</span>
        <button data-testid={`bump-${id}`} onClick={() => setN(n + 1)}>
          {n}
        </button>
        <button
          data-testid={`go-${id}`}
          onClick={() => route.navigate('meetings/m2/notes')}
        />
      </div>
    );
  };
}

vi.mock('./subapps', async () => {
  const { createRegistry } = await import('@qrtz/core');
  const app = (id: string, load?: () => Promise<void>) =>
    defineSubApp({
      manifest: { id, name: id, icon: id },
      load: async () => {
        await load?.();
        return { default: view(id) };
      },
    });
  return {
    registry: createRegistry([
      app('note'),
      app('talk', () => talkLoaded),
      app('settings'),
    ]),
  };
});

const { routes } = await import('./App');

async function boot(entry: string) {
  const router = createMemoryRouter(routes, { initialEntries: [entry] });
  const el = document.createElement('div');
  document.body.appendChild(el);
  const root = createRoot(el);
  await act(async () => root.render(<RouterProvider router={router} />));
  await act(async () => {
    await new Promise((r) => setTimeout(r, 10));
  });
  const q = (id: string) =>
    el.querySelector<HTMLElement>(`[data-testid="${id}"]`);
  const host = (id: string) =>
    el.querySelector<HTMLElement>(`[data-subapp="${id}"]`);
  const click = async (id: string) => {
    await act(async () => q(id)!.click());
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
  };
  return { router, el, root, q, host, click };
}

describe('shell routing (ADR 0091)', () => {
  it('a deep link opens the meeting tab; tab switches keep every visited app mounted with its state and route; the first visit to a lazy app keeps the old one on screen', async () => {
    const { router, root, q, host, click } = await boot(
      '/note/meetings/m1/summary',
    );
    expect(q('path-note')!.textContent).toBe('meetings/m1/summary');
    expect(q('active-note')!.textContent).toBe('true');
    await click('bump-note');

    // talk's module is still loading: note stays on screen (no blank frame) until it can show.
    await act(async () => q('nav-talk')!.click());
    expect(q('view-note')).not.toBeNull();
    expect(host('note')!.hidden).toBe(false);
    // (A Suspense fallback would hide it with display: none.)
    expect(host('note')!.style.display).not.toBe('none');
    expect(host('talk')).toBeNull();
    await act(async () => {
      releaseTalk();
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(router.state.location.pathname).toBe('/talk');
    expect(host('talk')!.hidden).toBe(false);
    expect(host('note')!.hidden).toBe(true);
    expect(host('note')!.hasAttribute('inert')).toBe(true);
    expect(q('active-note')!.textContent).toBe('false');
    // Hidden note keeps its last route.
    expect(q('path-note')!.textContent).toBe('meetings/m1/summary');

    for (let i = 0; i < 3; i++) {
      await click('nav-settings');
      await click('nav-note');
      await click('nav-talk');
    }
    // Back on note: same meeting and tab, same state, never remounted.
    await click('nav-note');
    expect(router.state.location.pathname).toBe('/note/meetings/m1/summary');
    expect(q('bump-note')!.textContent).toBe('1');
    expect(mounts).toEqual({ note: 1, talk: 1, settings: 1 });
    act(() => root.unmount());
  });

  it('unknown routes go to the default app; navigation requests go through the router with the meeting', async () => {
    const { router, root, q, click } = await boot('/nope/x');
    expect(router.state.location.pathname).toBe('/note');
    expect(q('path-note')!.textContent).toBe('');

    await act(async () =>
      requestNavigate({ app: 'note', meetingId: 'm9', tab: 'notes' }),
    );
    expect(router.state.location.pathname).toBe('/note/meetings/m9/notes');
    expect(q('path-note')!.textContent).toBe('meetings/m9/notes');

    await act(async () => requestNavigate({ app: 'settings' }));
    expect(router.state.location.pathname).toBe('/settings');

    // A hidden app's navigate (an effect, e.g. a recording starting) updates its own remembered route and never switches apps.
    await click('go-note');
    expect(router.state.location.pathname).toBe('/settings');
    expect(q('path-note')!.textContent).toBe('meetings/m2/notes');
    await click('nav-note');
    expect(router.state.location.pathname).toBe('/note/meetings/m2/notes');

    // The visible app's navigate is a real navigation.
    await act(async () => router.navigate('/note/meetings/m1/bogus'));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(router.state.location.pathname).toBe('/note/meetings/m1');
    act(() => root.unmount());
  });
});
