import type { ButtonHTMLAttributes, CSSProperties } from 'react';
import { useTheme } from './theme';
import { Spinner } from './Spinner';

export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger';
export type ButtonSize = 'sm' | 'md' | 'lg';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  loading?: boolean;
}

export function Button({
  variant = 'primary',
  size = 'md',
  loading,
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
    sm: { height: 28, padding: `0 ${t.space[3]}`, fontSize: t.font.size.sm },
    md: { height: 34, padding: `0 ${t.space[4]}`, fontSize: t.font.size.md },
    lg: { height: 40, padding: `0 ${t.space[5]}`, fontSize: t.font.size.md },
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
          fontFamily: 'inherit',
          fontWeight: t.font.weight.medium,
          whiteSpace: 'nowrap',
          transition: `opacity ${t.motion.fast} ${t.motion.easeOut}, background ${t.motion.fast} ${t.motion.easeOut}`,
          WebkitAppRegion: 'no-drag',
          ...v[variant],
          ...s[size],
          ...style,
        } as CSSProperties
      }
    >
      {loading ? <Spinner size={14} /> : null}
      {children}
    </button>
  );
}
