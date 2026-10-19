import {
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
} from 'react';
import { useTheme } from './theme';
import { Icon, type IconName } from './Icon';
import { IconButton } from './Layout';
import { Text } from './Text';
import { transition } from './tokens';

export interface MenuItem {
  id: string;
  label: string;
  icon?: IconName;
  /** A destructive item (remove): drawn in the danger color. */
  danger?: boolean;
  disabled?: boolean;
  onSelect: () => void;
}

/**
 * An icon button that opens a small menu of actions (an installed app's "more actions"). Keyboard: Enter, Space
 * or ArrowDown open it on the first item, ArrowUp on the last; ArrowUp/ArrowDown/Home/End move; Enter or Space
 * picks; Escape closes and returns focus to the button; Tab or a click outside closes. Clicks and keys inside
 * never reach a clickable row around it. Item ids become data-testids `<data-testid>-<id>`.
 */
export function Menu({
  label,
  icon = 'more-horizontal',
  items,
  badge,
  'data-testid': testId,
}: {
  /** The button's accessible name ("more actions for echo"). */
  label: string;
  icon?: IconName;
  items: MenuItem[];
  /** A dot on the button (an update is waiting). */
  badge?: boolean;
  'data-testid'?: string;
}) {
  const t = useTheme();
  const [open, setOpen] = useState(false);
  const [focusAt, setFocusAt] = useState<'first' | 'last'>('first');
  const button = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const root = useRef<HTMLSpanElement>(null);
  const id = useId();
  const enabled = items.filter((i) => !i.disabled);

  const entries = () => [
    ...(list.current?.querySelectorAll<HTMLButtonElement>(
      '[role="menuitem"]:not([disabled])',
    ) ?? []),
  ];
  useEffect(() => {
    if (!open) return;
    const e = entries();
    (focusAt === 'last' ? e[e.length - 1] : e[0])?.focus();
    const away = (ev: Event) => {
      if (!root.current?.contains(ev.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', away);
    return () => document.removeEventListener('mousedown', away);
  }, [open, focusAt]);

  const close = (refocus: boolean) => {
    setOpen(false);
    if (refocus) button.current?.focus();
  };
  const onKey = (e: KeyboardEvent) => {
    // Keys inside the menu belong to it: a row (or a modal) around it never sees them.
    e.stopPropagation();
    if (!open) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        setFocusAt(e.key === 'ArrowUp' ? 'last' : 'first');
        setOpen(true);
      }
      return;
    }
    const e2 = entries();
    const i = e2.indexOf(document.activeElement as HTMLButtonElement);
    const move = (n: number) => {
      e.preventDefault();
      e2[(n + e2.length) % e2.length]?.focus();
    };
    if (e.key === 'Escape') {
      e.preventDefault();
      close(true);
    } else if (e.key === 'Tab') close(false);
    else if (e.key === 'ArrowDown') move(i + 1);
    else if (e.key === 'ArrowUp') move(i < 0 ? -1 : i - 1);
    else if (e.key === 'Home') move(0);
    else if (e.key === 'End') move(-1);
  };
  const stop = (e: MouseEvent) => e.stopPropagation();

  return (
    <span
      ref={root}
      onClick={stop}
      onKeyDown={onKey}
      style={{ position: 'relative', display: 'inline-flex' }}
    >
      <IconButton
        ref={button}
        icon={icon}
        label={label}
        badge={badge}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        disabled={!enabled.length}
        onClick={() => {
          setFocusAt('first');
          setOpen((o) => !o);
        }}
        data-testid={testId}
      />
      {open ? (
        <div
          ref={list}
          id={id}
          role="menu"
          aria-label={label.toLowerCase()}
          data-testid={testId ? `${testId}-menu` : undefined}
          style={{
            position: 'absolute',
            top: 'calc(100% + 4px)',
            right: 0,
            zIndex: t.z.overlay,
            minWidth: 180,
            display: 'grid',
            gap: 2,
            padding: t.space[1],
            background: t.color.surface,
            border: `1px solid ${t.color.borderStrong}`,
            borderRadius: t.radius.md,
            boxShadow: '0 8px 24px rgba(0,0,0,0.35)',
          }}
        >
          {items.map((it) => (
            <button
              key={it.id}
              type="button"
              role="menuitem"
              tabIndex={-1}
              disabled={it.disabled}
              data-testid={testId ? `${testId}-${it.id}` : undefined}
              data-danger={it.danger ? '' : undefined}
              onClick={(e) => {
                e.stopPropagation();
                close(true);
                it.onSelect();
              }}
              style={{
                all: 'unset',
                boxSizing: 'border-box',
                cursor: it.disabled ? 'not-allowed' : 'pointer',
                opacity: it.disabled ? 0.5 : 1,
                display: 'flex',
                alignItems: 'center',
                gap: t.space[2],
                height: 28,
                padding: `0 ${t.space[2]}`,
                borderRadius: t.radius.sm,
                color: it.danger ? t.color.danger : t.color.text,
                transition: transition('background', 'hover', t),
              }}
            >
              {it.icon ? <Icon name={it.icon} size="sm" /> : null}
              <Text variant="bodySm" tone={it.danger ? 'danger' : 'default'}>
                {it.label.toLowerCase()}
              </Text>
            </button>
          ))}
        </div>
      ) : null}
    </span>
  );
}
