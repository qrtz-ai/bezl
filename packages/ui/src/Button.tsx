import { lowerTitle } from './case';
import { typeStyle } from './tokens';
import type { ButtonHTMLAttributes, CSSProperties } from 'react';
import { useTheme } from './theme';
import { Spinner } from './Spinner';
import { Icon, type IconName } from './Icon';

export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger';
export type ButtonSize = 'sm' | 'md' | 'lg';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  loading?: boolean;
  /** Leading Feather icon. */
  icon?: IconName;
  /** Fill the container's width (a card's action, so every card's button is the same size and shape). */
  block?: boolean;
}

export function Button({
  variant = 'secondary',
  size = 'md',
  loading,
  icon,
  block,
  disabled,
  style,
  children,
  ...rest
}: ButtonProps) {
  const t = useTheme();
  const v: Record<ButtonVariant, CSSProperties> = {
    primary: {
      background: t.color.primary,
      color: t.color.primaryText,
      borderColor: t.color.primary,
    },
    secondary: {
      background: t.color.accent,
      color: t.color.text,
      borderColor: t.color.border,
    },
    ghost: {
      background: 'transparent',
      color: t.color.text,
      borderColor: t.color.border,
    },
    danger: {
      background: t.color.dangerSolid,
      color: t.color.dangerSolidText,
      borderColor: t.color.dangerSolid,
    },
  };
  const s: Record<ButtonSize, CSSProperties> = {
    sm: { height: 28, padding: `0 ${t.space[3]}`, ...typeStyle('label', t) },
    md: { height: 34, padding: `0 ${t.space[4]}`, ...typeStyle('label', t) },
    lg: { height: 40, padding: `0 ${t.space[5]}`, ...typeStyle('label', t) },
  };
  const off = disabled || loading;
  return (
    <button
      type="button"
      disabled={off}
      aria-busy={loading || undefined}
      data-variant={variant}
      {...rest}
      style={
        {
          display: 'inline-flex',
          // A long label stays inside its container: clipped, never wider than it (QRT-304).
          maxWidth: '100%',
          overflow: 'hidden',
          alignItems: 'center',
          justifyContent: 'center',
          gap: t.space[2],
          cursor: off ? 'not-allowed' : 'pointer',
          opacity: off ? 0.55 : 1,
          borderRadius: t.radius.md,
          border: '1px solid',
          whiteSpace: 'nowrap',
          transition: `opacity ${t.motion.fast} ${t.motion.easeOut}, background ${t.motion.fast} ${t.motion.easeOut}`,
          WebkitAppRegion: 'no-drag',
          ...v[variant],
          ...s[size],
          ...(block ? { width: '100%', boxSizing: 'border-box' } : null),
          ...style,
        } as CSSProperties
      }
    >
      {loading ? (
        <Spinner size={14} />
      ) : icon ? (
        <Icon
          name={icon}
          size="sm"
          fill={icon === 'circle' || icon === 'stop'}
        />
      ) : null}
      {lowerTitle(children)}
    </button>
  );
}

/**
 * How many primary buttons a user can act on; a view has at most one (design principles: one primary action).
 * Buttons under `inert` (content blocked behind a required-model banner) don't count.
 */
export function primaryButtons(root: ParentNode): number {
  return [...root.querySelectorAll('button[data-variant="primary"]')].filter(
    (b) => !b.closest('[inert]'),
  ).length;
}
