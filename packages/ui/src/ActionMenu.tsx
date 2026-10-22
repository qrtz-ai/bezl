import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import { useTheme } from './theme';
import { MenuEntry, menuPanelStyle, type MenuItem } from './Menu';

export type Action = MenuItem;

/**
 * A right-click (context) menu of actions for whatever it wraps. Opens at the pointer on right-click, or from the
 * keyboard (Shift+F10 or the Menu key) at the focused element; it keeps itself inside the window. ArrowUp/ArrowDown/
 * Home/End move, Enter or Space picks, Escape closes and returns focus to the element it opened from; a click
 * outside, Tab, a scroll or the window losing focus close it. Same panel and items as Menu; danger items use the
 * error tone. Item ids become data-testids `<data-testid>-<id>`, the panel `<data-testid>-menu`.
 */
export function ActionMenu({
  actions,
  label = 'actions',
  children,
  'data-testid': testId,
}: {
  actions: Action[];
  /** The menu's accessible name. */
  label?: string;
  /** The trigger: its focusable element is where keyboard opening anchors and where focus returns. */
  children: ReactNode;
  'data-testid'?: string;
}) {
  const t = useTheme();
  const [at, setAt] = useState<{ x: number; y: number } | null>(null);
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);
  const list = useRef<HTMLDivElement>(null);
  const from = useRef<HTMLElement | null>(null);
  const id = useId();
  const open = at !== null;

  const entries = () => [
    ...(list.current?.querySelectorAll<HTMLButtonElement>(
      '[role="menuitem"]:not([disabled])',
    ) ?? []),
  ];
  const close = (refocus: boolean) => {
    setAt(null);
    setPos(null);
    if (refocus) from.current?.focus();
  };
  const openAt = (x: number, y: number, el: HTMLElement) => {
    from.current = el;
    setAt({ x, y });
  };
  const anchorOf = (target: EventTarget) =>
    ((target as HTMLElement).closest?.(
      '[tabindex], button, a, input',
    ) as HTMLElement | null) ?? (target as HTMLElement);

  // Keep the panel inside the window: flip left/up when it would cross the right/bottom edge.
  useLayoutEffect(() => {
    if (!at || !list.current) return;
    const r = list.current.getBoundingClientRect();
    const edge = parseFloat(t.space[1]);
    const x = Math.max(
      edge,
      at.x + r.width > window.innerWidth - edge ? at.x - r.width : at.x,
    );
    const y = Math.max(
      edge,
      at.y + r.height > window.innerHeight - edge ? at.y - r.height : at.y,
    );
    setPos({ x, y });
  }, [at, t]);
  // Focus the first item once the panel is placed (it is hidden until then, and hidden things take no focus).
  const placed = pos !== null;
  useEffect(() => {
    if (placed) entries()[0]?.focus();
  }, [placed]);
  useEffect(() => {
    if (!open) return;
    const away = (ev: Event) => {
      if (!list.current?.contains(ev.target as Node)) close(false);
    };
    const gone = () => close(false);
    document.addEventListener('mousedown', away);
    window.addEventListener('scroll', gone, true);
    window.addEventListener('blur', gone);
    window.addEventListener('resize', gone);
    return () => {
      document.removeEventListener('mousedown', away);
      window.removeEventListener('scroll', gone, true);
      window.removeEventListener('blur', gone);
      window.removeEventListener('resize', gone);
    };
  }, [open]);

  const onContextMenu = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const el = anchorOf(e.target);
    // A keyboard-raised contextmenu (no pointer) anchors at the element, like Shift+F10.
    if (e.clientX === 0 && e.clientY === 0) {
      const r = el.getBoundingClientRect();
      openAt(r.left, r.bottom, el);
    } else openAt(e.clientX, e.clientY, el);
  };
  const onTriggerKey = (e: KeyboardEvent) => {
    if (open) return;
    if (e.key === 'ContextMenu' || (e.key === 'F10' && e.shiftKey)) {
      e.preventDefault();
      e.stopPropagation();
      const el = anchorOf(e.target);
      const r = el.getBoundingClientRect();
      openAt(r.left, r.bottom, el);
    }
  };
  const onMenuKey = (e: KeyboardEvent) => {
    e.stopPropagation();
    const all = entries();
    const i = all.indexOf(document.activeElement as HTMLButtonElement);
    const move = (n: number) => {
      e.preventDefault();
      all[(n + all.length) % all.length]?.focus();
    };
    if (e.key === 'Escape') {
      e.preventDefault();
      close(true);
    } else if (e.key === 'Tab') {
      e.preventDefault();
      close(true);
    } else if (e.key === 'ArrowDown') move(i + 1);
    else if (e.key === 'ArrowUp') move(i < 0 ? -1 : i - 1);
    else if (e.key === 'Home') move(0);
    else if (e.key === 'End') move(-1);
  };

  return (
    <>
      <span
        style={{ display: 'contents' }}
        onContextMenu={onContextMenu}
        onKeyDown={onTriggerKey}
      >
        {children}
      </span>
      {open
        ? createPortal(
            <div
              ref={list}
              id={id}
              role="menu"
              aria-label={label.toLowerCase()}
              data-testid={testId ? `${testId}-menu` : undefined}
              onKeyDown={onMenuKey}
              onContextMenu={(e) => e.preventDefault()}
              style={{
                ...menuPanelStyle(t),
                position: 'fixed',
                left: (pos ?? at).x,
                top: (pos ?? at).y,
                zIndex: t.z.tooltip,
                visibility: pos ? 'visible' : 'hidden',
              }}
            >
              {actions.map((a) => (
                <MenuEntry
                  key={a.id}
                  item={a}
                  data-testid={testId ? `${testId}-${a.id}` : undefined}
                  onPick={() => close(true)}
                />
              ))}
            </div>,
            document.body,
          )
        : null}
    </>
  );
}
