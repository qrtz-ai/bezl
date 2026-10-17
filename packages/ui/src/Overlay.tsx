import { typeStyle } from './tokens';
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
  type TextareaHTMLAttributes,
} from 'react';
import { lowerTitle } from './case';
import { useTheme } from './theme';
import { Text } from './Text';

/** Modal dialog: backdrop, Esc / backdrop-click to close, focus moves into the panel. */
export function Dialog({
  open,
  onClose,
  title,
  children,
  width = 480,
  top = '18vh',
  label,
}: {
  open: boolean;
  onClose: () => void;
  title?: ReactNode;
  children: ReactNode;
  width?: number;
  top?: string;
  label?: string;
}) {
  const t = useTheme();
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const prev = document.activeElement as HTMLElement | null;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener('keydown', onKey);
    const f = ref.current?.querySelector<HTMLElement>(
      'input, textarea, button, [tabindex]',
    );
    (f ?? ref.current)?.focus();
    return () => {
      window.removeEventListener('keydown', onKey);
      prev?.focus?.();
    };
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div
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
        paddingTop: top,
        animation: `q-fade-in ${t.motion.fast} ${t.motion.easeOut}`,
      }}
    >
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-label={
          label ?? (typeof title === 'string' ? title.toLowerCase() : undefined)
        }
        tabIndex={-1}
        data-q-no-ring
        style={{
          width,
          maxWidth: 'calc(100vw - 32px)',
          background: t.color.surface,
          border: `1px solid ${t.color.borderStrong}`,
          borderRadius: t.radius.lg,
          boxShadow: '0 16px 48px rgba(0,0,0,0.5)',
          outline: 'none',
          overflow: 'hidden',
        }}
      >
        {title ? (
          <div
            style={{
              padding: `${t.space[4]} ${t.space[5]}`,
              borderBottom: `1px solid ${t.color.border}`,
            }}
          >
            <Text variant="h3">{lowerTitle(title)}</Text>
          </div>
        ) : null}
        {children}
      </div>
    </div>
  );
}

type ToastKind = 'info' | 'success' | 'error';
export interface ToastOptions {
  /** e.g. Undo. Clicking it runs onClick and dismisses the toast. */
  action?: { label: string; onClick: () => void };
  durationMs?: number;
}
interface ToastItem extends ToastOptions {
  id: number;
  kind: ToastKind;
  message: string;
}
const ToastContext = createContext<
  (m: string, kind?: ToastKind, opts?: ToastOptions) => void
>(() => {});
export const useToast = () => useContext(ToastContext);

/** Bottom-right toasts; auto-dismiss after 3.5 s. */
export function ToastProvider({ children }: { children: ReactNode }) {
  const t = useTheme();
  const [items, setItems] = useState<ToastItem[]>([]);
  const n = useRef(0);
  const dismiss = useCallback(
    (id: number) => setItems((xs) => xs.filter((x) => x.id !== id)),
    [],
  );
  const push = useCallback(
    (message: string, kind: ToastKind = 'info', opts: ToastOptions = {}) => {
      const id = ++n.current;
      setItems((xs) => [...xs, { id, kind, message, ...opts }]);
      setTimeout(() => dismiss(id), opts.durationMs ?? 3500);
    },
    [dismiss],
  );
  const dot = {
    info: t.color.textMuted,
    success: t.color.success,
    error: t.color.danger,
  };
  return (
    <ToastContext.Provider value={push}>
      {children}
      <div
        aria-live="polite"
        style={{
          position: 'fixed',
          right: 16,
          bottom: 16,
          zIndex: t.z.toast,
          display: 'grid',
          gap: 8,
        }}
      >
        {items.map((i) => (
          <div
            key={i.id}
            role="status"
            data-testid="toast"
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 10,
              minWidth: 240,
              padding: '10px 14px',
              background: t.color.surface,
              border: `1px solid ${t.color.borderStrong}`,
              borderRadius: t.radius.md,
              animation: `q-fade-in ${t.motion.normal} ${t.motion.easeOut}`,
            }}
          >
            <span
              style={{
                width: 6,
                height: 6,
                borderRadius: 3,
                background: dot[i.kind],
              }}
            />
            <Text variant="bodySm">{i.message}</Text>
            {i.action ? (
              <button
                type="button"
                data-testid="toast-action"
                onClick={() => {
                  i.action!.onClick();
                  dismiss(i.id);
                }}
                style={{
                  all: 'unset',
                  cursor: 'pointer',
                  marginLeft: 'auto',
                  ...typeStyle('label', t),
                  color: t.color.text,
                  textDecoration: 'underline',
                  borderRadius: t.radius.sm,
                }}
              >
                {i.action.label}
              </button>
            ) : null}
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

/** Hover/focus tooltip (CSS-positioned above the child). */
export function Tooltip({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  const t = useTheme();
  const [show, setShow] = useState(false);
  return (
    <span
      style={{ position: 'relative', display: 'inline-flex' }}
      onMouseEnter={() => setShow(true)}
      onMouseLeave={() => setShow(false)}
      onFocus={() => setShow(true)}
      onBlur={() => setShow(false)}
    >
      {children}
      {show ? (
        <span
          role="tooltip"
          style={{
            position: 'absolute',
            bottom: 'calc(100% + 6px)',
            left: '50%',
            transform: 'translateX(-50%)',
            whiteSpace: 'nowrap',
            padding: '4px 8px',
            ...typeStyle('caption', t),
            color: t.color.text,
            background: t.color.accent,
            border: `1px solid ${t.color.borderStrong}`,
            borderRadius: t.radius.sm,
            zIndex: t.z.tooltip,
            pointerEvents: 'none',
          }}
        >
          {label}
        </span>
      ) : null}
    </span>
  );
}

export function Switch({
  checked,
  onChange,
  label,
  showLabel,
  disabled,
  'data-testid': testId,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  /** Accessible name; drawn next to the switch only with `showLabel`. */
  label?: string;
  showLabel?: boolean;
  disabled?: boolean;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      data-testid={testId}
      onClick={() => onChange(!checked)}
      style={{
        all: 'unset',
        cursor: disabled ? 'not-allowed' : 'pointer',
        display: 'inline-flex',
        alignItems: 'center',
        gap: 8,
        opacity: disabled ? 0.5 : 1,
      }}
    >
      <span
        style={{
          width: 32,
          height: 18,
          borderRadius: 9,
          background: checked ? t.color.primary : t.color.borderStrong,
          position: 'relative',
          transition: `background ${t.motion.fast}`,
        }}
      >
        <span
          style={{
            position: 'absolute',
            top: 2,
            left: checked ? 16 : 2,
            width: 14,
            height: 14,
            borderRadius: 7,
            background: checked ? t.color.primaryText : t.color.text,
            transition: `left ${t.motion.fast} ${t.motion.easeOut}`,
          }}
        />
      </span>
      {label && showLabel ? <Text variant="bodySm">{label}</Text> : null}
    </button>
  );
}

export function TextArea({
  style,
  ...rest
}: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  const t = useTheme();
  return (
    <textarea
      {...rest}
      style={{
        width: '100%',
        minHeight: 80,
        padding: `${t.space[2]} ${t.space[3]}`,
        borderRadius: t.radius.md,
        border: `1px solid ${t.color.border}`,
        background: t.color.bg,
        color: t.color.text,
        ...typeStyle('body', t),
        resize: 'vertical',
        ...style,
      }}
    />
  );
}

/** Selectable list row (sidebar items, palette results, meeting lists). */
export function ListItem({
  active,
  onClick,
  children,
  right,
  ...rest
}: {
  active?: boolean;
  onClick?: () => void;
  children: ReactNode;
  right?: ReactNode;
} & Record<`data-${string}`, string>) {
  const t = useTheme();
  const [hover, setHover] = useState(false);
  return (
    <div
      role="option"
      aria-selected={!!active}
      onClick={onClick}
      tabIndex={onClick ? 0 : undefined}
      onKeyDown={
        onClick
          ? (e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                onClick();
              }
            }
          : undefined
      }
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      {...rest}
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: t.space[3],
        padding: `${t.space[2]} ${t.space[3]}`,
        borderRadius: t.radius.md,
        cursor: onClick ? 'pointer' : 'default',
        background: active
          ? t.color.accent
          : hover
            ? t.color.surfaceHover
            : 'transparent',
        transition: `background ${t.motion.fast}`,
      }}
    >
      <div style={{ flex: 1, minWidth: 0 }}>{children}</div>
      {right}
    </div>
  );
}
