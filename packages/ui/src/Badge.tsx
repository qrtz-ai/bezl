import { typeStyle, tokens } from './tokens';
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
  role,
  'data-testid': testId,
}: {
  children: ReactNode;
  tone?: BadgeTone;
  role?: 'status' | 'alert';
  'data-testid'?: string;
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
      data-testid={testId}
      role={role}
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: tokens.space[1],
        padding: `${tokens.space[0.5]} ${tokens.space[2]}`,
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
