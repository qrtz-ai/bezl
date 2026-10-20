// @vitest-environment jsdom
// ADR 0231 finding 3: a link in a Dash payload must never give the opened page a way back into the app window
// (no window.opener, no Referer). Dash links are not navigable elements: they render as buttons and open only
// through the host (`dash:open-link` -> the default browser, a separate process). Any anchor must carry
// rel="noopener noreferrer" and no target that opens a webview window.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import type { DashWidget } from '@qrtz/core';
import { ThemeProvider } from '@qrtz/ui';
import { Block, DASH_LINK_REL } from './blocks';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  root = null;
  document.body.innerHTML = '';
});

const listWidget = {
  key: 'links',
  type: 'list',
  title: 'links',
  state: {
    items: [{ text: 'evil', url: 'https://evil.example/x' }, { text: 'plain' }],
  },
  events: [],
} as unknown as DashWidget;

describe('dash payload links (ADR 0231 finding 3)', () => {
  it('rel constant is noopener noreferrer', () => {
    expect(DASH_LINK_REL.split(' ').sort()).toEqual(['noopener', 'noreferrer']);
  });

  it('render with no href/target the webview could follow, and open only via the host', async () => {
    const el = document.createElement('div');
    document.body.appendChild(el);
    const open = vi.fn(async () => true);
    const winOpen = vi.spyOn(window, 'open').mockImplementation(() => null);
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    root = createRoot(el);
    act(() =>
      root!.render(
        <ThemeProvider>
          <Block widget={listWidget} open={open} />
        </ThemeProvider>,
      ),
    );
    expect(el.textContent).toContain('evil');
    for (const a of el.querySelectorAll('a')) {
      expect(a.getAttribute('rel')?.split(' ')).toEqual(
        expect.arrayContaining(['noopener', 'noreferrer']),
      );
      expect(a.getAttribute('target')).not.toBe('_blank');
    }
    expect(el.querySelectorAll('[href], [target], [formtarget]').length).toBe(
      0,
    );
    const btn = [...el.querySelectorAll('button')].find(
      (b) => b.textContent === 'evil',
    )!;
    await act(async () => btn.click());
    expect(open).toHaveBeenCalledWith('https://evil.example/x');
    expect(winOpen).not.toHaveBeenCalled();
  });
});
