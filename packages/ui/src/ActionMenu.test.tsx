// @vitest-environment jsdom
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ActionMenu } from './ActionMenu';
import { TalkHistoryRow } from './Talk';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;
let cleanup: (() => void) | undefined;
afterEach(() => cleanup?.());

function render(node: React.ReactNode) {
  const el = document.createElement('div');
  document.body.append(el);
  const root = createRoot(el);
  act(() => root.render(node));
  cleanup = () => {
    act(() => root.unmount());
    el.remove();
  };
}
const q = (id: string) =>
  document.querySelector<HTMLElement>(`[data-testid="${id}"]`);
const rightClick = (el: HTMLElement, x = 5, y = 5) =>
  act(() => {
    el.dispatchEvent(
      new MouseEvent('contextmenu', {
        bubbles: true,
        cancelable: true,
        clientX: x,
        clientY: y,
      }),
    );
  });
const key = (el: Element, k: string, shiftKey = false) =>
  act(() => {
    el.dispatchEvent(
      new KeyboardEvent('keydown', { key: k, shiftKey, bubbles: true }),
    );
  });

function mountMenu() {
  const a = vi.fn();
  const b = vi.fn();
  render(
    <ActionMenu
      data-testid="m"
      actions={[
        { id: 'a', label: 'Alpha', onSelect: a },
        { id: 'b', label: 'beta', danger: true, onSelect: b },
      ]}
    >
      <button type="button" data-testid="trigger">
        x
      </button>
    </ActionMenu>,
  );
  return { trigger: q('trigger')!, a, b };
}

describe('ActionMenu', () => {
  it('opens at the pointer on right-click with role menu/menuitem and lowercase labels', () => {
    const { trigger } = mountMenu();
    expect(q('m-menu')).toBeNull();
    rightClick(trigger, 40, 30);
    const menu = q('m-menu')!;
    expect(menu.getAttribute('role')).toBe('menu');
    expect(menu.style.position).toBe('fixed');
    expect(menu.style.left).toBe('40px');
    expect(menu.style.top).toBe('30px');
    expect(q('m-a')!.getAttribute('role')).toBe('menuitem');
    expect(q('m-a')!.textContent).toBe('alpha');
    expect(document.activeElement).toBe(q('m-a'));
  });
  it('keeps itself inside the window edges', () => {
    const { trigger } = mountMenu();
    rightClick(trigger, window.innerWidth - 1, window.innerHeight - 1);
    const m = q('m-menu')!;
    expect(parseFloat(m.style.left)).toBeLessThanOrEqual(window.innerWidth);
    expect(parseFloat(m.style.top)).toBeLessThanOrEqual(window.innerHeight);
    expect(parseFloat(m.style.left)).toBeGreaterThanOrEqual(0);
  });
  it('opens from the keyboard with Shift+F10 and the Menu key', () => {
    const { trigger } = mountMenu();
    trigger.focus();
    key(trigger, 'F10', true);
    expect(q('m-menu')).not.toBeNull();
    key(q('m-menu')!, 'Escape');
    expect(q('m-menu')).toBeNull();
    expect(document.activeElement).toBe(trigger);
    key(trigger, 'ContextMenu');
    expect(q('m-menu')).not.toBeNull();
  });
  it('arrow keys move and wrap; picking runs the action, closes and refocuses the trigger', () => {
    const { trigger, b } = mountMenu();
    trigger.focus();
    rightClick(trigger);
    key(q('m-menu')!, 'ArrowDown');
    expect(document.activeElement).toBe(q('m-b'));
    key(q('m-menu')!, 'ArrowDown');
    expect(document.activeElement).toBe(q('m-a'));
    key(q('m-menu')!, 'End');
    act(() => (document.activeElement as HTMLElement).click());
    expect(b).toHaveBeenCalledTimes(1);
    expect(q('m-menu')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });
  it('closes on an outside click, scroll and window blur', () => {
    const { trigger } = mountMenu();
    for (const close of [
      () =>
        document.body.dispatchEvent(
          new MouseEvent('mousedown', { bubbles: true }),
        ),
      () => window.dispatchEvent(new Event('scroll')),
      () => window.dispatchEvent(new Event('blur')),
    ]) {
      rightClick(trigger);
      expect(q('m-menu')).not.toBeNull();
      act(close);
      expect(q('m-menu')).toBeNull();
    }
  });
  it('draws danger items in the error tone', () => {
    const { trigger } = mountMenu();
    rightClick(trigger);
    expect(q('m-b')!.hasAttribute('data-danger')).toBe(true);
    expect(q('m-a')!.hasAttribute('data-danger')).toBe(false);
  });
});

describe('echo history entry', () => {
  function mount() {
    const onCopy = vi.fn();
    const onDelete = vi.fn();
    render(
      <ul>
        <TalkHistoryRow
          text="hello there"
          meta="now"
          onCopy={onCopy}
          onDelete={onDelete}
        />
      </ul>,
    );
    return { entry: q('talk-entry')!, onCopy, onDelete };
  }
  it('has no inline buttons; the entry is focusable, named by its text, with a right-click hint', () => {
    const { entry } = mount();
    expect(document.querySelectorAll('button')).toHaveLength(0);
    expect(entry.tabIndex).toBe(0);
    expect(entry.getAttribute('role')).toBe('button');
    expect(entry.textContent).toContain('hello there');
    const hint = document.getElementById(
      entry.getAttribute('aria-describedby')!,
    );
    expect(hint?.textContent).toContain('right-click');
  });
  it('click, Enter and Space copy', () => {
    const { entry, onCopy } = mount();
    act(() => entry.click());
    key(entry, 'Enter');
    key(entry, ' ');
    expect(onCopy).toHaveBeenCalledTimes(3);
  });
  it('right-click shows copy and delete (danger) under the old test ids', () => {
    const { entry, onCopy, onDelete } = mount();
    expect(q('talk-entry-copy')).toBeNull();
    rightClick(entry);
    expect(q('talk-entry-copy')?.textContent).toBe('copy');
    expect(q('talk-entry-delete')?.hasAttribute('data-danger')).toBe(true);
    act(() => q('talk-entry-delete')!.click());
    expect(onDelete).toHaveBeenCalledTimes(1);
    expect(onCopy).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(entry);
  });
});
