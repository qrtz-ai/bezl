import { typeStyle, tokens } from './tokens';
import {
  createContext,
  useCallback,
  useContext,
  useRef,
  useState,
  type ReactNode,
  type TextareaHTMLAttributes,
} from 'react';
import { lowerTitle } from './case';
import { useTheme } from './theme';
import { Text } from './Text';
import { SCRIM_CLASS, useModalLayer } from './Modal';
import { Switch as ShadcnSwitch } from './components/ui/switch';
import { cn } from './lib/utils';
import { Dialog as DialogRoot, DialogContent } from './components/ui/dialog';

/**
 * A small modal dialog (a consent, a confirm) on shadcn's Dialog: backdrop, Esc / backdrop-click to close, focus
 * moves into the panel and stays there. Same layering as Modal: inline, our focus trap and Escape stack.
 */
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
  const ref = useRef<HTMLDivElement>(null);
  const [node, setNode] = useState<HTMLDivElement | null>(null);
  const setRef = useCallback((n: HTMLDivElement | null) => {
    ref.current = n;
    setNode(n);
  }, []);
  useModalLayer(ref, open && !!node, onClose);
  if (!open) return null;
  return (
    <DialogRoot modal={false} open onOpenChange={() => {}}>
      <div
        onMouseDown={(e) => e.target === e.currentTarget && onClose()}
        className={SCRIM_CLASS}
        style={{ paddingTop: top }}
      >
        <DialogContent
          inline
          ref={setRef}
          aria-modal="true"
          aria-label={
            label ??
            (typeof title === 'string' ? title.toLowerCase() : undefined)
          }
          aria-describedby={undefined}
          tabIndex={-1}
          data-q-no-ring
          onOpenAutoFocus={(e) => e.preventDefault()}
          onCloseAutoFocus={(e) => e.preventDefault()}
          onEscapeKeyDown={(e) => e.preventDefault()}
          onInteractOutside={(e) => e.preventDefault()}
          className="tw:block tw:max-w-[calc(100vw-32px)] tw:gap-0 tw:overflow-hidden tw:border-border-strong tw:bg-popover tw:p-0"
          style={{ width }}
        >
          {title ? (
            <div className="tw:border-b tw:border-border tw:px-5 tw:py-4">
              <Text variant="h3">{lowerTitle(title)}</Text>
            </div>
          ) : null}
          {children}
        </DialogContent>
      </div>
    </DialogRoot>
  );
}

/**
 * Toasts are for non-critical confirmations only ("copied", "deleted · undo"). Errors never toast: they show
 * inline where the action happened (ErrorCallout, QRT-303), so there is no 'error' kind.
 */
type ToastKind = 'info' | 'success';
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
      if ((kind as string) === 'error')
        throw new Error('toasts never carry errors; show an ErrorCallout');
      const id = ++n.current;
      setItems((xs) => [...xs, { id, kind, message, ...opts }]);
      setTimeout(() => dismiss(id), opts.durationMs ?? 3500);
    },
    [dismiss],
  );
  const dot = {
    info: t.color.textMuted,
    success: t.color.success,
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
          gap: tokens.space[2],
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
              gap: tokens.space[3],
              minWidth: 240,
              padding: `${tokens.space[2]} ${tokens.space[3]}`,
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
                borderRadius: tokens.radius.sm,
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
            padding: `${tokens.space[1]} ${tokens.space[2]}`,
            ...typeStyle('caption', t),
            color: t.color.text,
            background: t.color.accent,
            border: `1px solid ${t.color.borderStrong}`,
            borderRadius: t.radius.sm,
            zIndex: t.z.tooltip,
            pointerEvents: 'none',
          }}
        >
          {lowerTitle(label)}
        </span>
      ) : null}
    </span>
  );
}

/** The switch (ctrl toggle and every on/off setting) on shadcn's Switch: role=switch, Space/Enter toggle. */
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
  const withLabel = !!(label && showLabel);
  const control = (
    <ShadcnSwitch
      checked={checked}
      onCheckedChange={onChange}
      aria-label={label}
      disabled={disabled}
      data-testid={testId}
      className={cn(
        'tw:h-[18px] tw:w-8 tw:p-0 tw:cursor-pointer tw:rounded-lg tw:border-0 tw:shadow-none tw:disabled:cursor-not-allowed tw:data-[state=unchecked]:bg-border-strong tw:dark:data-[state=unchecked]:bg-border-strong',
        // With a label the wrapper dims both; the switch must not dim twice.
        withLabel && 'tw:disabled:opacity-100',
      )}
      thumbClassName="tw:size-3.5! tw:rounded-md tw:data-[state=checked]:translate-x-4 tw:data-[state=unchecked]:translate-x-0.5 tw:data-[state=checked]:bg-primary-foreground tw:data-[state=unchecked]:bg-foreground"
    />
  );
  if (!withLabel) return control;
  return (
    <span
      className={cn(
        'tw:inline-flex tw:items-center tw:gap-2',
        disabled ? 'tw:cursor-not-allowed tw:opacity-50' : 'tw:cursor-pointer',
      )}
      onClick={(e) => {
        // The visible label toggles too, as it did when the label sat inside the switch button.
        if (!disabled && !(e.target as Element).closest('[role="switch"]'))
          onChange(!checked);
      }}
    >
      {control}
      <Text variant="bodySm">{label}</Text>
    </span>
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
