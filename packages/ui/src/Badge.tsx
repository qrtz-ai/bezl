import { typeStyle } from './tokens';
import type { ReactNode } from 'react';
import { useTheme } from './theme';

export type BadgeTone =
  | 'neutral'
  | 'primary'
  | 'success'
  | 'warning'
  | 'danger';

/** qrtz.ai badge: tinted bg (10%), tinted border (20%), xs semibold. */
export function Badge({
  children,
  tone = 'primary',
}: {
  children: ReactNode;
  tone?: BadgeTone;
}) {
  const t = useTheme();
  const c = {
    neutral: t.color.textMuted,
    primary: t.color.primary,
    success: t.color.success,
    warning: t.color.warning,
    danger: t.color.danger,
  }[tone];
  return (
    <span
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 4,
        padding: '2px 8px',
        ...typeStyle('label', t),
        color: c,
        background: `color-mix(in srgb, ${c} 10%, transparent)`,
        border: `1px solid color-mix(in srgb, ${c} 20%, transparent)`,
        borderRadius: t.radius.sm,
      }}
    >
      {children}
    </span>
  );
}
