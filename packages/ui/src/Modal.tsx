import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from 'react';
import {
  Dialog as DialogRoot,
  DialogContent,
  DialogTitle,
} from './components/ui/dialog';
import { cn } from './lib/utils';
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
  /** `first` (default): the `[data-autofocus]` control, else the first control. `dialog`: the dialog itself
   * unless a control is marked `[data-autofocus]` (the opts modal, so its close button opens without a ring). */
  initialFocus: 'first' | 'dialog' = 'first',
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
    const first =
      el.querySelector<HTMLElement>('[data-autofocus]') ??
      (initialFocus === 'first'
        ? el.querySelector<HTMLElement>('input, textarea, button, [tabindex]')
        : null);
    (first ?? el).focus();
    return () => {
      window.removeEventListener('keydown', onKey);
      const i = layers.lastIndexOf(el);
      if (i >= 0) layers.splice(i, 1);
      prev?.focus?.();
    };
  }, [open, ref, initialFocus]);
}

/** The scrim behind a Modal or Dialog (shadcn's overlay look, owned here because both render inline). */
export const SCRIM_CLASS =
  'tw:fixed tw:inset-0 tw:z-50 tw:flex tw:items-start tw:justify-center tw:box-border tw:bg-overlay tw:backdrop-blur-[2px] tw:animate-in';

/**
 * A large modal (an app's detail on the qrtz page) on shadcn's Dialog: a dimmed scrim (click it to close), a title
 * row with a close button, and a body that scrolls vertically inside the window (never sideways).
 *
 * Focus, the focus trap and Escape stay on the shared layer stack (useModalLayer), not Radix's: Radix's trap stole
 * focus from a Dialog opened over a modal (a consent), and its Escape closed both layers at once. It renders
 * inline (no Portal) so a modal left open in a hidden (inert) sub app stays hidden. Radix supplies the dialog
 * semantics and title labelling.
 */
export function Modal({
  open,
  onClose,
  title,
  children,
  width = 640,
  closeLabel = 'close',
  initialFocus = 'first',
  'data-testid': testId,
}: {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  children: ReactNode;
  width?: number;
  closeLabel?: string;
  /** Where focus goes on open; see `useModalLayer`. */
  initialFocus?: 'first' | 'dialog';
  'data-testid'?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [node, setNode] = useState<HTMLDivElement | null>(null);
  const setRef = useCallback((n: HTMLDivElement | null) => {
    ref.current = n;
    setNode(n);
  }, []);
  useModalLayer(ref, open && !!node, onClose, initialFocus);
  if (!open) return null;
  return (
    <DialogRoot modal={false} open onOpenChange={() => {}}>
      <div
        data-testid={testId ? `${testId}-scrim` : undefined}
        data-modal-scrim=""
        onMouseDown={(e) => e.target === e.currentTarget && onClose()}
        className={cn(SCRIM_CLASS, 'tw:px-4 tw:pb-4 tw:pt-[10vh]')}
      >
        <DialogContent
          inline
          ref={setRef}
          aria-modal="true"
          aria-describedby={undefined}
          tabIndex={-1}
          data-q-no-ring
          data-testid={testId}
          onOpenAutoFocus={(e) => e.preventDefault()}
          onCloseAutoFocus={(e) => e.preventDefault()}
          onEscapeKeyDown={(e) => e.preventDefault()}
          onInteractOutside={(e) => e.preventDefault()}
          className="tw:flex tw:max-h-[80vh] tw:max-w-full tw:flex-col tw:gap-0 tw:overflow-hidden tw:border-border tw:bg-popover tw:p-0"
          style={{ width }}
        >
          <div className="tw:flex tw:min-w-0 tw:flex-none tw:items-center tw:gap-2 tw:pt-5 tw:pr-4 tw:pb-2 tw:pl-5">
            <DialogTitle asChild>
              <span className="tw:min-w-0 tw:flex-1">
                <Text variant="h3" truncate>
                  {lowerTitle(title)}
                </Text>
              </span>
            </DialogTitle>
            <IconButton
              icon="close"
              variant="ghost"
              label={closeLabel}
              onClick={onClose}
              data-testid={testId ? `${testId}-close` : undefined}
            />
          </div>
          <div className="tw:min-h-0 tw:min-w-0 tw:overflow-x-hidden tw:overflow-y-auto tw:px-5 tw:pt-3 tw:pb-5">
            {children}
          </div>
        </DialogContent>
      </div>
    </DialogRoot>
  );
}
