import type { ButtonHTMLAttributes } from 'react';
import { useTheme } from './theme';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'primary' | 'ghost';
}

export function Button({ variant = 'primary', style, ...rest }: ButtonProps) {
  const t = useTheme();
  const primary = variant === 'primary';
  return (
    <button
      type="button"
      {...rest}
      style={{
        cursor: 'pointer',
        padding: `${t.space[2]} ${t.space[4]}`,
        borderRadius: t.radius.sm,
        border: `1px solid ${primary ? t.color.accent : t.color.border}`,
        background: primary ? t.color.accent : 'transparent',
        color: primary ? t.color.accentText : t.color.text,
        fontFamily: 'inherit',
        fontSize: t.font.size.md,
        ...style,
      }}
    />
  );
}
