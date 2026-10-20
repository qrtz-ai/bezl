// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act, useEffect } from 'react';
import { createRoot } from 'react-dom/client';
import { renderToStaticMarkup } from 'react-dom/server';
import { ThemeProvider } from './theme';
import { ToastProvider, useToast } from './Overlay';
import { CaptureStatus } from './Lifecycle';
import { isReadable, userError } from './UserError';

describe('readable errors (QRT-303)', () => {
  it('leads with what happened and keeps codes under details', () => {
    expect(userError(new Error('llm http 429'), 'summarize')).toEqual({
      title: "couldn't summarize",
      next: 'try again in a moment.',
      detail: 'llm http 429',
    });
    const plain = userError(new Error('the folder is read-only'), 'save');
    expect(plain.next).toBe('the folder is read-only');
    expect(plain.detail).toBeNull();
    expect(isReadable('EACCES: permission denied')).toBe(false);
  });

  it('never toasts an error', () => {
    const el = document.createElement('div');
    const root = createRoot(el);
    let thrown: unknown = null;
    function Fire() {
      const toast = useToast();
      useEffect(() => {
        try {
          (toast as (m: string, k: string) => void)('boom', 'error');
        } catch (e) {
          thrown = e;
        }
      }, [toast]);
      return null;
    }
    act(() =>
      root.render(
        <ThemeProvider mode="dark">
          <ToastProvider>
            <Fire />
          </ToastProvider>
        </ThemeProvider>,
      ),
    );
    expect(String(thrown)).toContain('never carry errors');
    root.unmount();
  });

  it("a degraded capture's main line has no codes, counts or statuses", () => {
    const html = renderToStaticMarkup(
      <ThemeProvider mode="dark">
        <CaptureStatus
          reconnecting={{ attempt: 2, secondsLeft: 12 }}
          degraded={[{ id: 'mic-only', label: 'mic only' }]}
          error="llm http 429"
        />
      </ThemeProvider>,
    );
    const main = html.split('<details')[0]!.replace(/<[^>]+>/g, ' ');
    expect(main).not.toMatch(/http|\d/);
    expect(html).toContain('llm http 429');
  });
});
