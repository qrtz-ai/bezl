// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { AppFrame, Page, SideNav, ThemeProvider } from './index';

describe('window drag regions (tauri)', () => {
  it('the frame has an invisible fixed drag strip; the empty sidebar drags; rows never do', async () => {
    const el = document.createElement('div');
    document.body.appendChild(el);
    const root = createRoot(el);
    await act(async () =>
      root.render(
        <ThemeProvider mode="dark">
          <AppFrame
            sidebar={
              <SideNav
                items={[{ id: 'apps', label: 'apps', group: 'foot' }]}
                active="apps"
                onSelect={() => {}}
              />
            }
          >
            <Page>x</Page>
          </AppFrame>
        </ThemeProvider>,
      ),
    );
    const strip = el.querySelector<HTMLElement>('.q-drag')!;
    expect(strip.getAttribute('data-tauri-drag-region')).toBe('');
    expect(strip.getAttribute('aria-hidden')).toBe('true');
    expect(strip.style.cssText).toBe('');
    expect(strip.childElementCount).toBe(0);
    expect(
      el.querySelector('nav')!.getAttribute('data-tauri-drag-region'),
    ).toBe('');
    // Pages have no header row of their own.
    expect(el.querySelector('[data-page-header]')).toBeNull();
    expect(
      el
        .querySelector('[data-testid="nav-apps"]')!
        .hasAttribute('data-tauri-drag-region'),
    ).toBe(false);
    act(() => root.unmount());
    el.remove();
  });
});
