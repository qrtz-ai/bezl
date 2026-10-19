// @vitest-environment jsdom
/** ADR 0218: nano's notes are a notes app: list newest first, search, new, debounced autosave, confirm delete. */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import type { MeetingNote, QrtzClient } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { Notes, SAVE_DEBOUNCE_MS, noteTitle, visibleNotes } from './Notes';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const note = (id: string, text: string, createdAt: number): MeetingNote => ({
  id,
  text,
  createdAt,
  atMs: 0,
});

function fakeClient(initial: MeetingNote[]) {
  let notes = [...initial];
  let next = 1;
  const calls: [string, unknown][] = [];
  const invoke = vi.fn(async (ch: string, req: Record<string, unknown>) => {
    calls.push([ch, req]);
    switch (ch) {
      case 'note:list-notes':
        return notes;
      case 'note:add-note': {
        const n = note(`n${next++}`, req.text as string, 1000 + next);
        notes = [...notes, n];
        return { ...n, meetingId: 'm1' };
      }
      case 'note:update-note':
        notes = notes.map((n) =>
          n.id === req.id ? { ...n, text: req.text as string } : n,
        );
        return { ok: true };
      case 'note:delete-note':
        notes = notes.filter((n) => n.id !== req.id);
        return { ok: true };
    }
    throw new Error(`unexpected ${ch}`);
  });
  const client = {
    subApp: 'note',
    invoke,
    on: () => () => {},
  } as unknown as QrtzClient;
  return {
    client,
    calls,
    get notes() {
      return notes;
    },
  };
}

const typeInto = (el: HTMLTextAreaElement | HTMLInputElement, v: string) => {
  const proto = Object.getPrototypeOf(el) as object;
  Object.getOwnPropertyDescriptor(proto, 'value')!.set!.call(el, v);
  el.dispatchEvent(new Event('input', { bubbles: true }));
};
const q = <T extends Element>(el: Element, id: string) =>
  el.querySelector<T>(`[data-testid="${id}"]`);
const flush = () =>
  act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });

let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  root = null;
  vi.useRealTimers();
  document.body.innerHTML = '';
});

async function mount(initial: MeetingNote[]) {
  const f = fakeClient(initial);
  const el = document.createElement('div');
  document.body.append(el);
  root = createRoot(el);
  await act(async () =>
    root!.render(
      <QrtzClientProvider client={f.client}>
        <Notes meetingId="m1" />
      </QrtzClientProvider>,
    ),
  );
  await flush();
  return { el, f };
}

describe('noteTitle / visibleNotes', () => {
  it('titles a note from its first non-empty line', () => {
    expect(noteTitle('\n  groceries \nmilk')).toBe('groceries');
    expect(noteTitle('   ')).toBe('untitled');
    expect(noteTitle('x'.repeat(80))).toHaveLength(60);
  });
  it('sorts newest first and filters case-insensitively', () => {
    const ns = [
      note('a', 'Alpha', 1),
      note('b', 'beta', 3),
      note('c', 'ALPHABET', 2),
    ];
    expect(visibleNotes(ns, '').map((n) => n.id)).toEqual(['b', 'c', 'a']);
    expect(visibleNotes(ns, 'alpha').map((n) => n.id)).toEqual(['c', 'a']);
  });
});

describe('Notes (notes app)', () => {
  it('shows the empty state, then lists notes newest first', async () => {
    const empty = await mount([]);
    expect(q(empty.el, 'empty-state')?.textContent).toContain('no notes yet');
    act(() => root?.unmount());
    const { el } = await mount([
      note('a', 'old\nbody', 1),
      note('b', 'new', 2),
    ]);
    const rows = [...el.querySelectorAll('[data-testid="note"]')];
    expect(rows.map((r) => r.textContent?.split('0:00')[0])).toEqual([
      'new',
      'old',
    ]);
  });

  it('a new note is created once on the first debounced save, then updated', async () => {
    vi.useFakeTimers();
    const { el, f } = await mount([]);
    await act(async () => q<HTMLButtonElement>(el, 'note-new')!.click());
    await flush();
    const ed = q<HTMLTextAreaElement>(el, 'note-editor')!;
    act(() => typeInto(ed, 'plan'));
    act(() => typeInto(ed, 'plan\nship it'));
    // Nothing is written while typing.
    expect(f.calls.filter(([c]) => c !== 'note:list-notes')).toEqual([]);
    await act(async () => vi.advanceTimersByTime(SAVE_DEBOUNCE_MS));
    await flush();
    act(() =>
      typeInto(
        q<HTMLTextAreaElement>(el, 'note-editor')!,
        'plan\nship it\ntoday',
      ),
    );
    await act(async () => vi.advanceTimersByTime(SAVE_DEBOUNCE_MS));
    await flush();
    const writes = f.calls.filter(([c]) => c !== 'note:list-notes');
    expect(writes).toEqual([
      ['note:add-note', { text: 'plan\nship it', meetingId: 'm1' }],
      ['note:update-note', { id: 'n1', text: 'plan\nship it\ntoday' }],
    ]);
    expect(q(el, 'note-status')?.textContent).toBe('saved');
  });

  it('search filters the list', async () => {
    const { el } = await mount([note('a', 'apples', 1), note('b', 'pears', 2)]);
    act(() => typeInto(q<HTMLInputElement>(el, 'notes-search')!, 'APP'));
    const rows = el.querySelectorAll('[data-testid="note"]');
    expect(rows).toHaveLength(1);
    expect(rows[0]!.textContent).toContain('apples');
  });

  it('delete asks first and only deletes on confirm', async () => {
    const { el, f } = await mount([note('a', 'keep or not', 1)]);
    await act(async () => q<HTMLButtonElement>(el, 'note')!.click());
    await flush();
    expect(q<HTMLTextAreaElement>(el, 'note-editor')!.value).toBe(
      'keep or not',
    );
    await act(async () => q<HTMLButtonElement>(el, 'note-delete')!.click());
    expect(f.calls.some(([c]) => c === 'note:delete-note')).toBe(false);
    await act(async () =>
      document
        .querySelector<HTMLButtonElement>(
          '[data-testid="confirm-delete-note-ok"]',
        )!
        .click(),
    );
    await flush();
    expect(f.calls).toContainEqual(['note:delete-note', { id: 'a' }]);
    expect(f.notes).toEqual([]);
  });

  it('switching notes saves the pending edit to the note it belongs to', async () => {
    vi.useFakeTimers();
    const { el, f } = await mount([note('a', 'one', 1), note('b', 'two', 2)]);
    const [b, a] = [
      ...el.querySelectorAll<HTMLButtonElement>('[data-testid="note"]'),
    ];
    await act(async () => a!.click());
    await flush();
    act(() => typeInto(q<HTMLTextAreaElement>(el, 'note-editor')!, 'one!'));
    await act(async () => b!.click());
    await flush();
    expect(f.calls).toContainEqual([
      'note:update-note',
      { id: 'a', text: 'one!' },
    ]);
    expect(q<HTMLTextAreaElement>(el, 'note-editor')!.value).toBe('two');
  });
});
