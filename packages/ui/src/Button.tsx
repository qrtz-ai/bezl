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
}

export function Button({
  variant = 'primary',
  size = 'md',
  loading,
  icon,
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
      background: '#dc2626', // 4.8:1 with white text (WCAG AA)
      color: '#fff',
      borderColor: '#dc2626',
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
      {...rest}
      style={
        {
          display: 'inline-flex',
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
