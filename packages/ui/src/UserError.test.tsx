// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act, useEffect, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { renderToStaticMarkup } from 'react-dom/server';
import { ThemeProvider } from './theme';
import { ToastProvider, useToast } from './Overlay';
import { CaptureStatus } from './Lifecycle';
import { isReadable, userError } from './UserError';
import { NotesApp } from './NotesApp';
import { ModelsPanel, type SpeechSetupView } from './Setup';

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

/** The text a person sees: everything except what sits inside a collapsed <details> or a tooltip. */
function visible(el: Element): string {
  const c = el.cloneNode(true) as Element;
  c.querySelectorAll('details').forEach((d) => d.remove());
  return c.textContent ?? '';
}

function mount(node: ReactNode) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const root = createRoot(el);
  act(() => root.render(<ThemeProvider mode="dark">{node}</ThemeProvider>));
  return {
    el,
    done: () => {
      act(() => root.unmount());
      el.remove();
    },
  };
}

describe('paths and os errors are technical (QRT-311 C1)', () => {
  it.each([
    'no such file or directory: /Users/x/y',
    'no such file or directory: /Users/jared/Library/Application Support/qrtz/notes.db',
    "couldn't open ~/Library/Logs/qrtz",
    'access is denied: \\\\server\\share\\notes',
    'access is denied: C:\\Users\\x',
    'permission denied (os error 13)',
    'Permission denied (OS error 1)',
    'not found (/tmp/a)',
  ])('%s goes under details', (raw) => {
    expect(isReadable(raw)).toBe(false);
    const e = userError(new Error(raw), 'save the note');
    expect(e.next).toBe('try again in a moment.');
    expect(e.detail).toBe(raw);
  });

  it.each([
    'the folder is read-only',
    'turn it on or off in opts',
    'pick a name with letters and/or numbers',
  ])('%s stays readable', (raw) => {
    expect(isReadable(raw)).toBe(true);
  });
});

describe('views keep the raw reason out of the visible text (QRT-311 C2)', () => {
  const raw = 'no such file or directory: /Users/x/y (os error 2)';
  const error = userError(new Error(raw), 'save the note');

  it('NotesApp shows the title and next step; the raw reason is only the tooltip', () => {
    const noop = () => {};
    const { el, done } = mount(
      <NotesApp
        notes={[]}
        selectedId="new"
        draft="hello"
        query=""
        error={error}
        onQuery={noop}
        onSelect={noop}
        onNew={noop}
        onDraft={noop}
        onDelete={noop}
      />,
    );
    const line = el.querySelector('[data-testid="notes-error"]')!;
    expect(line.textContent).toBe(
      "couldn't save the note: try again in a moment.",
    );
    expect(line.getAttribute('title')).toBe(raw);
    expect(el.textContent).not.toContain('/Users');
    expect(el.textContent).not.toContain('os error');
    done();
  });

  it('ModelsPanel shows the title and next step; the raw reason is only under details', () => {
    const noop = () => {};
    const { el, done } = mount(
      <ModelsPanel
        status={
          {
            configured: true,
            installed: true,
            bytes: 1_000_000,
            latestBytes: 1_000_000,
            download: { state: 'idle' },
          } as unknown as SpeechSetupView
        }
        sets={[{ name: 'parakeet' }]}
        error={userError(new Error(raw), 'download the models')}
        onDownload={noop}
        onPause={noop}
        onResume={noop}
        onDelete={noop}
      />,
    );
    const box = el.querySelector('[data-testid="models-error"]')!;
    expect(visible(box)).toContain("couldn't download the models");
    expect(visible(box)).toContain('try again in a moment.');
    expect(visible(el)).not.toContain('/Users');
    expect(visible(el)).not.toContain('os error');
    expect(
      el.querySelector('[data-testid="models-error-details"]')?.textContent,
    ).toContain(raw);
    done();
  });
});
