import { typeStyle } from './tokens';
import type { ReactNode } from 'react';
import { useTheme } from './theme';
import { Badge as ShadcnBadge } from './components/ui/badge';
import { cn } from './lib/utils';

export type BadgeTone =
  | 'neutral'
  | 'primary'
  | 'success'
  | 'warning'
  | 'danger';

const TONE: Record<BadgeTone, string> = {
  neutral: 'tw:[--badge:var(--q-color-text-muted)]',
  primary: 'tw:[--badge:var(--q-color-primary)]',
  success: 'tw:[--badge:var(--q-color-success)]',
  warning: 'tw:[--badge:var(--q-color-warning)]',
  danger: 'tw:[--badge:var(--q-color-danger)]',
};

/**
 * qrtz.ai badge on shadcn's outline Badge: tinted bg (10%), tinted border (20%), label type, sm radius. It wraps and
 * shrinks like the old span (shadcn's nowrap, shrink-0 and centering are off).
 */
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
  return (
    <ShadcnBadge
      variant="outline"
      data-testid={testId}
      role={role}
      className={cn(
        TONE[tone],
        'tw:w-auto tw:shrink tw:justify-start tw:overflow-visible tw:rounded-sm tw:whitespace-normal tw:border-[color-mix(in_srgb,var(--badge)_20%,transparent)] tw:bg-[color-mix(in_srgb,var(--badge)_10%,transparent)] tw:text-(--badge) tw:transition-none',
      )}
      style={typeStyle('label', t)}
    >
      {children}
    </ShadcnBadge>
  );
}
