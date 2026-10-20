// @vitest-environment jsdom
// Bozniak hardening item 7: payload rendering is text only (no HTML, no markdown-to-HTML), and a link must pass
// isHttpsUrl: javascript:, data:, http: and friends are refused by the schema and by openLink.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { DashBlocks, isHttpsUrl, type DashWidget } from '@qrtz/core';
import { ThemeProvider } from '@qrtz/ui';
import { Block, openLink } from './blocks';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const BAD = [
  'javascript:alert(1)',
  'JavaScript:alert(1)',
  ' javascript:alert(1)',
  'data:text/html,<script>alert(1)</script>',
  'http://example.com/',
  'vbscript:x',
  'file:///etc/passwd',
  'https://user@evil.example/',
  'https://evil.example/\njavascript:x',
];

let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  root = null;
  document.body.innerHTML = '';
});
const render = (widget: DashWidget) => {
  const el = document.createElement('div');
  document.body.appendChild(el);
  root = createRoot(el);
  act(() =>
    root!.render(
      <ThemeProvider>
        <Block widget={widget} open={async () => true} />
      </ThemeProvider>,
    ),
  );
  return el;
};

describe('dash payload rendering (Bozniak item 7)', () => {
  it('isHttpsUrl, the list schema and openLink refuse javascript:, data: and non-https links', async () => {
    const open = vi.fn(async () => true);
    for (const u of BAD) {
      expect(isHttpsUrl(u), u).toBe(false);
      expect(
        DashBlocks.list.safeParse({ items: [{ text: 'x', url: u }] }).success,
        u,
      ).toBe(false);
      expect(await openLink(u, open, () => true), u).toBe(false);
    }
    expect(open).not.toHaveBeenCalled();
  });

  it('a list with a bad link renders nothing clickable', () => {
    const el = render({
      key: 'l',
      type: 'list',
      title: 'l',
      state: { items: [{ text: 'x', url: 'javascript:alert(1)' }] },
      events: [],
    } as unknown as DashWidget);
    expect(el.querySelectorAll('a, button').length).toBe(0);
  });

  it('html and markdown in agent text are shown as text, never as elements', () => {
    // The schema already refuses html and images; what it allows is still never turned into elements.
    expect(DashBlocks.markdown.safeParse({ text: '<b>x</b>' }).success).toBe(
      false,
    );
    const text = '**bold** _em_ [l](javascript:alert(1)) `code`';
    const el = render({
      key: 'm',
      type: 'markdown',
      title: 'm',
      state: { text },
      events: [],
    } as unknown as DashWidget);
    expect(el.querySelectorAll('img, b, a, strong, script').length).toBe(0);
    expect(el.querySelectorAll('em, code').length).toBe(0);
    expect(el.textContent).toContain('[l](javascript:alert(1))');
  });
});
