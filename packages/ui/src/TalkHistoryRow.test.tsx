// @vitest-environment jsdom
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { TalkHistoryRow } from './Talk';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;
let cleanup: (() => void) | undefined;
afterEach(() => cleanup?.());

function mount() {
  const onCopy = vi.fn();
  const onDelete = vi.fn();
  const el = document.createElement('ul');
  document.body.append(el);
  const root = createRoot(el);
  act(() =>
    root.render(
      <TalkHistoryRow
        text="hello there"
        meta="now"
        onCopy={onCopy}
        onDelete={onDelete}
      />,
    ),
  );
  cleanup = () => {
    act(() => root.unmount());
    el.remove();
  };
  const entry = document.querySelector<HTMLElement>(
    '[data-testid="talk-entry"]',
  )!;
  return { entry, onCopy, onDelete };
}
const q = (id: string) =>
  document.querySelector<HTMLElement>(`[data-testid="${id}"]`);
const openMenu = (entry: HTMLElement) =>
  act(() => {
    entry.dispatchEvent(
      new MouseEvent('contextmenu', {
        bubbles: true,
        cancelable: true,
        clientX: 5,
        clientY: 5,
      }),
    );
  });

describe('echo history entry', () => {
  it('has no inline copy or delete buttons and is a focusable, named control', () => {
    const { entry } = mount();
    expect(document.querySelectorAll('button')).toHaveLength(0);
    expect(entry.tabIndex).toBe(0);
    expect(entry.getAttribute('role')).toBe('button');
    // The name is the visible text (WCAG 2.5.3); the description carries the hint.
    expect(entry.getAttribute('aria-label')).toBeNull();
    expect(entry.getAttribute('aria-description')).toContain('click to copy');
    expect(entry.getAttribute('aria-description')).toContain('right-click');
  });
  it('click copies', () => {
    const { entry, onCopy } = mount();
    act(() => entry.click());
    expect(onCopy).toHaveBeenCalledTimes(1);
  });
  it('enter and space copy', () => {
    const { entry, onCopy } = mount();
    for (const key of ['Enter', ' '])
      act(() => {
        entry.dispatchEvent(
          new KeyboardEvent('keydown', { key, bubbles: true }),
        );
      });
    expect(onCopy).toHaveBeenCalledTimes(2);
  });
  it('right-click opens a context menu with copy and delete (destructive)', () => {
    const { entry, onCopy } = mount();
    expect(q('talk-entry-copy')).toBeNull();
    openMenu(entry);
    expect(q('talk-entry-menu')?.getAttribute('role')).toBe('menu');
    expect(q('talk-entry-copy')?.textContent).toBe('copy');
    expect(q('talk-entry-delete')?.getAttribute('data-variant')).toBe(
      'destructive',
    );
    expect(onCopy).not.toHaveBeenCalled();
  });
  it('delete from the menu deletes', () => {
    const { entry, onDelete, onCopy } = mount();
    openMenu(entry);
    act(() => q('talk-entry-delete')!.click());
    expect(onDelete).toHaveBeenCalledTimes(1);
    expect(onCopy).not.toHaveBeenCalled();
  });
});
