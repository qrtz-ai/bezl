// @vitest-environment jsdom
/** ADR 0219/0220: nano's notes app over doc:* (list, search, new standalone note, autosave, conflicts, delete). */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { AppError, type Doc, type QrtzClient } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { Notes, SAVE_DEBOUNCE_MS, noteTitle } from './Notes';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const doc = (
  id: string,
  body: string,
  updatedAt: number,
  meetings: string[] = [],
): Doc => ({
  id,
  body,
  updatedAt,
  meetings,
  title: noteTitle(body),
  path: `${id}.md`,
  hash: `h-${id}-0`,
});

function fake(initial: Doc[]) {
  const docs = new Map(initial.map((d) => [d.id, d]));
  let next = 1;
  let rev = 1;
  const calls: [string, unknown][] = [];
  let emit: (p: { ids: string[] }) => void = () => {};
  const invoke = vi.fn(
    async (ch: string, req: Record<string, unknown> = {}) => {
      calls.push([ch, req]);
      switch (ch) {
        case 'doc:list':
          return [...docs.values()]
            .filter((d) => !req.query || d.body.includes(req.query as string))
            .filter(
              (d) =>
                !req.meetingId || d.meetings.includes(req.meetingId as string),
            )
            .sort((a, b) => b.updatedAt - a.updatedAt);
        case 'doc:get':
          return docs.get(req.id as string);
        case 'doc:create': {
          const d = doc(
            `d${next++}`,
            '',
            100 + next,
            req.meetingId ? [req.meetingId as string] : [],
          );
          docs.set(d.id, d);
          return d;
        }
        case 'doc:save': {
          const d = docs.get(req.id as string)!;
          if (d.hash !== req.baseHash && !req.force)
            throw new AppError('E_VALIDATION', 'changed', {
              details: { conflict: true },
            });
          const n = {
            ...d,
            body: req.body as string,
            hash: `h-${d.id}-${rev++}`,
          };
          docs.set(d.id, n);
          return n;
        }
        case 'doc:trash':
          docs.delete(req.id as string);
          return { ok: true };
      }
      throw new Error(`unexpected ${ch}`);
    },
  );
  const client = {
    subApp: 'note',
    invoke,
    on: (ev: string, cb: (p: { ids: string[] }) => void) => {
      if (ev === 'doc:changed') emit = cb;
      return () => {};
    },
  } as unknown as QrtzClient;
  return { client, calls, docs, emit: (ids: string[]) => emit({ ids }) };
}

const typeInto = (el: HTMLTextAreaElement | HTMLInputElement, v: string) => {
  const proto = Object.getPrototypeOf(el) as object;
  Object.getOwnPropertyDescriptor(proto, 'value')!.set!.call(el, v);
  el.dispatchEvent(new Event('input', { bubbles: true }));
};
const q = <T extends Element>(el: Element, id: string) =>
  el.querySelector<T>(`[data-testid="${id}"]`);
const settle = () =>
  act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
const writes = (calls: [string, unknown][]) =>
  calls.filter(([c]) => c !== 'doc:list' && c !== 'doc:get');

let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  root = null;
  vi.useRealTimers();
  document.body.innerHTML = '';
});

async function mount(initial: Doc[], props: { meetingId?: string } = {}) {
  const f = fake(initial);
  const el = document.createElement('div');
  document.body.append(el);
  root = createRoot(el);
  await act(async () =>
    root!.render(
      <QrtzClientProvider client={f.client}>
        <Notes {...props} />
      </QrtzClientProvider>,
    ),
  );
  await settle();
  return { el, f };
}

describe('noteTitle', () => {
  it('is the first non-empty line without heading marks', () => {
    expect(noteTitle('\n  # groceries \nmilk')).toBe('groceries');
    expect(noteTitle('   ')).toBe('untitled');
    expect(noteTitle('x'.repeat(80))).toHaveLength(60);
  });
});

describe('Notes (doc:*)', () => {
  it('empty state, then every note newest first', async () => {
    const empty = await mount([]);
    expect(q(empty.el, 'empty-state')?.textContent).toContain('no notes yet');
    act(() => root?.unmount());
    const { el } = await mount([doc('a', 'old', 1), doc('b', 'new', 2)]);
    const rows = [...el.querySelectorAll('[data-testid="note"]')];
    expect(rows.map((r) => r.textContent?.slice(0, 3))).toEqual(['new', 'old']);
  });

  it('a new standalone note is created on the first debounced save, then updated', async () => {
    vi.useFakeTimers();
    const { el, f } = await mount([]);
    await act(async () => q<HTMLButtonElement>(el, 'note-new')!.click());
    await settle();
    act(() => typeInto(q<HTMLTextAreaElement>(el, 'note-editor')!, 'plan'));
    expect(writes(f.calls)).toEqual([]);
    await act(async () => vi.advanceTimersByTime(SAVE_DEBOUNCE_MS));
    await settle();
    act(() =>
      typeInto(q<HTMLTextAreaElement>(el, 'note-editor')!, 'plan\nship'),
    );
    await act(async () => vi.advanceTimersByTime(SAVE_DEBOUNCE_MS));
    await settle();
    expect(writes(f.calls)).toEqual([
      ['doc:create', {}],
      ['doc:save', { id: 'd1', body: 'plan', baseHash: 'h-d1-0' }],
      ['doc:save', { id: 'd1', body: 'plan\nship', baseHash: 'h-d1-1' }],
    ]);
    expect(q(el, 'note-status')?.textContent).toBe('saved');
  });

  it("a meeting's notes: only its notes, and a new one is linked to it", async () => {
    vi.useFakeTimers();
    const { el, f } = await mount(
      [doc('a', 'mine', 1, ['m1']), doc('b', 'other', 2)],
      { meetingId: 'm1' },
    );
    expect(el.querySelectorAll('[data-testid="note"]')).toHaveLength(1);
    await act(async () => q<HTMLButtonElement>(el, 'note-new')!.click());
    await settle();
    act(() => typeInto(q<HTMLTextAreaElement>(el, 'note-editor')!, 'x'));
    await act(async () => vi.advanceTimersByTime(SAVE_DEBOUNCE_MS));
    await settle();
    expect(f.calls).toContainEqual(['doc:create', { meetingId: 'm1' }]);
  });

  it('an outside edit reloads a clean note, and a stale save asks instead of overwriting', async () => {
    vi.useFakeTimers();
    const { el, f } = await mount([doc('a', 'one', 1)]);
    await act(async () => q<HTMLButtonElement>(el, 'note')!.click());
    await settle();
    // Changed on disk while clean: reloaded.
    f.docs.set('a', { ...f.docs.get('a')!, body: 'from vim', hash: 'h-ext-1' });
    await act(async () => f.emit(['a']));
    await settle();
    expect(q<HTMLTextAreaElement>(el, 'note-editor')!.value).toBe('from vim');
    // Changed again while typing: the save is refused, the user chooses.
    act(() =>
      typeInto(q<HTMLTextAreaElement>(el, 'note-editor')!, 'from vim + mine'),
    );
    f.docs.set('a', { ...f.docs.get('a')!, body: 'from ia', hash: 'h-ext-2' });
    await act(async () => vi.advanceTimersByTime(SAVE_DEBOUNCE_MS));
    await settle();
    expect(q(el, 'note-conflict')).not.toBeNull();
    expect(f.docs.get('a')!.body).toBe('from ia');
    const keep = [...q(el, 'note-conflict')!.querySelectorAll('button')].find(
      (b) => b.textContent === 'keep mine',
    )!;
    await act(async () => keep.click());
    await settle();
    expect(f.calls).toContainEqual([
      'doc:save',
      { id: 'a', body: 'from vim + mine', baseHash: 'h-ext-1', force: true },
    ]);
    expect(q(el, 'note-conflict')).toBeNull();
  });

  it('search asks the vault', async () => {
    const { el, f } = await mount([
      doc('a', 'apples', 1),
      doc('b', 'pears', 2),
    ]);
    vi.useFakeTimers();
    act(() => typeInto(q<HTMLInputElement>(el, 'notes-search')!, 'apples'));
    await act(async () => vi.advanceTimersByTime(200));
    await settle();
    expect(f.calls).toContainEqual(['doc:list', { query: 'apples' }]);
    expect(el.querySelectorAll('[data-testid="note"]')).toHaveLength(1);
  });

  it('delete asks first and moves the note to the trash on confirm', async () => {
    const { el, f } = await mount([doc('a', 'keep or not', 1)]);
    await act(async () => q<HTMLButtonElement>(el, 'note')!.click());
    await settle();
    await act(async () => q<HTMLButtonElement>(el, 'note-delete')!.click());
    expect(writes(f.calls)).toEqual([]);
    await act(async () =>
      document
        .querySelector<HTMLButtonElement>(
          '[data-testid="confirm-delete-note-ok"]',
        )!
        .click(),
    );
    await settle();
    expect(f.calls).toContainEqual(['doc:trash', { id: 'a' }]);
  });
});
