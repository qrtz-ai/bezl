import { cn } from './lib/utils';
import type { ReactNode } from 'react';
import { Badge as ShadcnBadge } from './components/ui/badge';

export type BadgeTone =
  | 'neutral'
  | 'primary'
  | 'success'
  | 'warning'
  | 'danger';

const TONE = {
  neutral: { variant: 'secondary', className: undefined },
  primary: { variant: 'default', className: undefined },
  success: { variant: 'outline', className: 'tw:text-(--q-color-success)' },
  warning: { variant: 'outline', className: 'tw:text-(--q-color-warning)' },
  danger: { variant: 'destructive', className: undefined },
} as const;

/** qrtz.ai badge: stock shadcn Badge; tones pick its variant (success and warning are outline in the tone color). */
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
  return (
    <ShadcnBadge
      variant={TONE[tone].variant}
      data-testid={testId}
      role={role}
      // The one non-stock class: a long badge clips inside its container (QRT-304).
      className={cn('tw:max-w-full', TONE[tone].className)}
    >
      {children}
    </ShadcnBadge>
  );
}
