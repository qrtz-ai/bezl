import {
  useEffect,
  useId,
  useRef,
  type ReactNode,
  type RefObject,
} from 'react';
import { useTheme } from './theme';
import { Text } from './Text';
import { IconButton } from './Layout';
import { lowerTitle } from './case';

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** The open modal layers, innermost last: only the top one takes Escape and Tab (a consent over a detail modal). */
const layers: HTMLElement[] = [];

const focusables = (el: HTMLElement) =>
  [...el.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    (n) => !n.closest('[inert]') && n.getAttribute('aria-hidden') !== 'true',
  );

/**
 * A modal layer: focus moves in when it opens and back to where it was when it closes, Tab and Shift+Tab stay
 * inside it (focus trap), and Escape closes only the topmost layer. A layer left open in a hidden (inert) sub
 * app never takes keys from the visible one.
 */
export function useModalLayer(
  ref: RefObject<HTMLElement | null>,
  open: boolean,
  onClose: () => void,
) {
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const el = ref.current;
    if (!open || !el) return;
    layers.push(el);
    const prev = document.activeElement as HTMLElement | null;
    const onKey = (e: KeyboardEvent) => {
      if (layers[layers.length - 1] !== el || el.closest('[inert]')) return;
      if (e.key === 'Escape') {
        e.preventDefault();
        close.current();
        return;
      }
      if (e.key !== 'Tab') return;
      const f = focusables(el);
      if (!f.length) {
        e.preventDefault();
        el.focus();
        return;
      }
      const first = f[0]!;
      const last = f[f.length - 1]!;
      const at = document.activeElement as HTMLElement | null;
      if (!at || !el.contains(at)) {
        e.preventDefault();
        first.focus();
      } else if (e.shiftKey && (at === first || at === el)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && at === last) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener('keydown', onKey);
    // The dialog itself takes focus unless a control asks for it, so the close button doesn't open with a ring.
    (el.querySelector<HTMLElement>('[data-autofocus]') ?? el).focus();
    return () => {
      window.removeEventListener('keydown', onKey);
      const i = layers.lastIndexOf(el);
      if (i >= 0) layers.splice(i, 1);
      prev?.focus?.();
    };
  }, [open, ref]);
}

/**
 * A large modal (an app's detail on the qrtz page): a dimmed scrim (click it to close), a title row with a close
 * button, and a body that scrolls vertically inside the window (never sideways). Esc closes; focus is trapped
 * inside while it is open.
 */
export function Modal({
  open,
  onClose,
  title,
  children,
  width = 640,
  closeLabel = 'close',
  'data-testid': testId,
}: {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  children: ReactNode;
  width?: number;
  closeLabel?: string;
  'data-testid'?: string;
}) {
  const t = useTheme();
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useModalLayer(ref, open, onClose);
  if (!open) return null;
  return (
    <div
      data-testid={testId ? `${testId}-scrim` : undefined}
      data-modal-scrim=""
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: t.z.dialog,
        background: t.color.overlay,
        backdropFilter: 'blur(2px)',
        display: 'flex',
        justifyContent: 'center',
        alignItems: 'flex-start',
        padding: '10vh 16px 16px',
        boxSizing: 'border-box',
        animation: `q-fade-in ${t.motion.fast} ${t.motion.easeOut}`,
      }}
    >
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        data-q-no-ring
        data-testid={testId}
        style={{
          width,
          maxWidth: '100%',
          maxHeight: '80vh',
          display: 'flex',
          flexDirection: 'column',
          background: t.color.surface,
          border: `1px solid ${t.color.border}`,
          borderRadius: t.radius.lg,
          boxShadow: '0 12px 32px rgba(0,0,0,0.24)',
          outline: 'none',
          overflow: 'hidden',
        }}
      >
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: t.space[2],
            flex: '0 0 auto',
            padding: `${t.space[5]} ${t.space[4]} ${t.space[2]} ${t.space[5]}`,
            minWidth: 0,
          }}
        >
          <span id={titleId} style={{ minWidth: 0, flex: 1 }}>
            <Text variant="h3" truncate>
              {lowerTitle(title)}
            </Text>
          </span>
          <IconButton
            icon="close"
            variant="ghost"
            label={closeLabel}
            onClick={onClose}
            data-testid={testId ? `${testId}-close` : undefined}
          />
        </div>
        <div
          style={{
            padding: `${t.space[3]} ${t.space[5]} ${t.space[5]}`,
            overflowY: 'auto',
            overflowX: 'hidden',
            minHeight: 0,
            minWidth: 0,
          }}
        >
          {children}
        </div>
      </div>
    </div>
  );
}
