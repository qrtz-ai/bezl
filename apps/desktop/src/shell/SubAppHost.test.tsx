// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act, Suspense, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { defineSubApp } from '@qrtz/core';
import { Dialog, ThemeProvider } from '@qrtz/ui';
import { SubAppHost } from './SubAppHost';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

describe('SubAppHost error boundary (M0.9)', () => {
  it('shows a fallback for a crashing sub app without taking down the shell', async () => {
    const boom = defineSubApp({
      manifest: { id: 'boom', name: 'Boom', icon: '!' },
      load: async () => ({
        default: () => {
          throw new Error('kaboom');
        },
      }),
    });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const root = createRoot(el);
    const origError = console.error;
    console.error = () => {};
    try {
      await act(async () => {
        root.render(
          <div>
            <nav data-testid="shell-nav">shell</nav>
            <Suspense fallback={null}>
              <SubAppHost app={boom} />
            </Suspense>
          </div>,
        );
      });
      await act(async () => {
        await new Promise((r) => setTimeout(r, 20));
      });
    } finally {
      console.error = origError;
    }
    expect(
      el.querySelector('[data-testid="subapp-error"]')?.textContent,
    ).toContain('kaboom');
    expect(el.querySelector('[data-testid="shell-nav"]')).not.toBeNull();
    root.unmount();
  });
});

describe('SubAppHost keep-mounted tabs', () => {
  const mounts: Record<string, number> = { a: 0, b: 0 };
  const lifecycle: string[] = [];
  const make = (id: string) =>
    defineSubApp({
      manifest: { id, name: id.toUpperCase(), icon: id },
      activate: () => {
        lifecycle.push(`on:${id}`);
        return () => lifecycle.push(`off:${id}`);
      },
      load: async () => ({
        default: function View() {
          const [n, setN] = useState(0);
          useEffect(() => {
            mounts[id]!++;
          }, []);
          return (
            <button data-testid={`count-${id}`} onClick={() => setN(n + 1)}>
              {id}:{n}
            </button>
          );
        },
      }),
    });
  const a = make('a');
  const b = make('b');

  it('switching tabs never remounts, keeps state, never shows an empty frame, and hides the inactive app as inert', async () => {
    const el = document.createElement('div');
    document.body.appendChild(el);
    const root = createRoot(el);
    const render = (active: 'a' | 'b') =>
      root.render(
        <Suspense fallback={null}>
          <SubAppHost app={a} active={active === 'a'} />
          <SubAppHost app={b} active={active === 'b'} />
        </Suspense>,
      );
    await act(async () => render('a'));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
    const btn = (id: string) =>
      el.querySelector<HTMLButtonElement>(`[data-testid="count-${id}"]`);
    act(() => btn('a')!.click());
    expect(btn('a')!.textContent).toBe('a:1');
    expect(lifecycle).toEqual(['on:a']);

    const host = (id: string) =>
      el.querySelector<HTMLElement>(`[data-subapp="${id}"]`)!;
    for (let i = 0; i < 3; i++) {
      // Each switch is a single synchronous commit: the target is already on screen, no blank frame.
      act(() => render('b'));
      expect(host('b').hidden).toBe(false);
      expect(btn('b')!.textContent).toBe('b:0');
      act(() => render('a'));
      expect(host('a').hidden).toBe(false);
      expect(btn('a')!.textContent).toBe('a:1');
    }
    expect(mounts).toEqual({ a: 1, b: 1 });
    // activate() is per visible session.
    expect(lifecycle.slice(0, 4)).toEqual(['on:a', 'off:a', 'on:b', 'off:b']);

    act(() => render('b'));
    const hidden = host('a');
    expect(hidden.hidden).toBe(true);
    expect(hidden.hasAttribute('inert')).toBe(true);
    expect(hidden.getAttribute('aria-hidden')).toBe('true');
    expect(host('b').hasAttribute('inert')).toBe(false);
    expect(host('b').hasAttribute('aria-hidden')).toBe(false);
    act(() => root.unmount());
    expect(lifecycle.at(-1)).toBe('off:b');
  });
});

describe('hidden sub app keyboard', () => {
  it('a dialog left open in a hidden sub app does not take Escape', async () => {
    let closed = 0;
    const withDialog = defineSubApp({
      manifest: { id: 'dlg', name: 'Dlg', icon: 'd' },
      load: async () => ({
        default: () => (
          <Dialog open onClose={() => closed++} label="sheet">
            sheet
          </Dialog>
        ),
      }),
    });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const root = createRoot(el);
    const render = (active: boolean) =>
      root.render(
        <ThemeProvider mode="light">
          <Suspense fallback={null}>
            <SubAppHost app={withDialog} active={active} />
          </Suspense>
        </ThemeProvider>,
      );
    await act(async () => render(true));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
    act(() => render(false));
    const esc = () =>
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    act(esc);
    expect(closed).toBe(0);
    act(() => render(true));
    act(esc);
    expect(closed).toBe(1);
    act(() => root.unmount());
  });
});

describe('SubAppHost own Suspense boundary', () => {
  it('a hidden sub app that suspends after loading never blanks the visible one', async () => {
    let release: () => void = () => {};
    const pending = new Promise<void>((r) => (release = r));
    let suspend = false;
    const hidden = defineSubApp({
      manifest: { id: 'sleepy', name: 'Sleepy', icon: 's' },
      load: async () => ({
        default: function Sleepy() {
          if (suspend) throw pending;
          return <span data-testid="sleepy">sleepy</span>;
        },
      }),
    });
    const shown = defineSubApp({
      manifest: { id: 'shown', name: 'Shown', icon: 's' },
      load: async () => ({
        default: () => <span data-testid="shown">shown</span>,
      }),
    });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const root = createRoot(el);
    const render = (tick: number) =>
      root.render(
        <Suspense fallback={<i data-testid="shell-fallback" />}>
          <SubAppHost app={hidden} active={false} path={String(tick)} />
          <SubAppHost app={shown} active />
        </Suspense>,
      );
    await act(async () => {
      render(0);
      await new Promise((r) => setTimeout(r, 20));
    });
    expect(el.querySelector('[data-testid="shown"]')).not.toBeNull();
    // The hidden app suspends (outside any transition) on a re-render.
    suspend = true;
    await act(async () => {
      render(1);
      await new Promise((r) => setTimeout(r, 20));
    });
    expect(el.querySelector('[data-testid="shell-fallback"]')).toBeNull();
    const shownEl = el.querySelector<HTMLElement>('[data-testid="shown"]');
    expect(shownEl).not.toBeNull();
    expect(
      shownEl!.closest<HTMLElement>('[style*="display: none"]'),
    ).toBeNull();
    suspend = false;
    await act(async () => {
      release();
      await pending;
    });
    root.unmount();
  });
});
