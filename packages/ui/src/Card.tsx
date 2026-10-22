import { Card as ShadcnCard } from './components/ui/card';
import { cn } from './lib/utils';
import { tokens } from './tokens';
import type { CSSProperties, HTMLAttributes, ReactNode } from 'react';
import { useTheme } from './theme';

/** A card column's minimum width in a CardGrid: as many columns as fit, never narrower than this. */
export const CARD_MIN_WIDTH = tokens.layout.cardMin;

/**
 * Cards in a real grid: `repeat(auto-fill, minmax(220px, 1fr))` columns, the spacing token gap, and equal-height
 * rows (every row as tall as the tallest card), so cards built with `fill` line their bottom actions up. The one
 * way cards are laid out (marketplace browse, the module manager).
 */
export function CardGrid({
  children,
  role = 'list',
  'data-testid': testId,
}: {
  children: ReactNode;
  role?: string;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <div
      role={role}
      data-testid={testId}
      data-card-grid=""
      style={{
        display: 'grid',
        gridTemplateColumns: `repeat(auto-fill, minmax(min(${CARD_MIN_WIDTH}px, 100%), 1fr))`,
        gridAutoRows: '1fr',
        alignItems: 'stretch',
        gap: t.space[4],
        minWidth: 0,
      }}
    >
      {children}
    </div>
  );
}

/**
 * One cell of a CardGrid: fills its cell, a flex column whose `footer` sits at the bottom (margin-top: auto) so
 * buttons line up across a row whatever the text above them.
 */
export function GridCard({
  children,
  footer,
  role = 'listitem',
  'data-testid': testId,
}: {
  children: ReactNode;
  footer?: ReactNode;
  role?: string;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <div
      role={role}
      data-testid={testId}
      data-grid-card=""
      style={{ display: 'flex', minWidth: 0, height: '100%' }}
    >
      <Card
        padding={4}
        style={{
          flex: 1,
          minWidth: 0,
          display: 'flex',
          flexDirection: 'column',
          gap: t.space[3],
        }}
      >
        {children}
        {footer ? (
          <div
            data-card-footer=""
            style={{
              marginTop: 'auto',
              display: 'flex',
              flexDirection: 'column',
              gap: t.space[2],
            }}
          >
            {footer}
          </div>
        ) : null}
      </Card>
    </div>
  );
}

const PAD = { 3: 'tw:p-3', 4: 'tw:p-4', 6: 'tw:p-6' } as const;

/** qrtz.ai card on shadcn Card: 1px border, rounded-lg, generous padding, no shadow, block flow (not shadcn's column). */
export function Card({
  children,
  padding = 6,
  interactive,
  selected,
  style,
  className,
  ...rest
}: {
  children: ReactNode;
  padding?: 3 | 4 | 6;
  interactive?: boolean;
  selected?: boolean;
  style?: CSSProperties;
} & HTMLAttributes<HTMLDivElement>) {
  const t = useTheme();
  return (
    <ShadcnCard
      {...rest}
      className={cn(
        'tw:block tw:gap-0 tw:border-border tw:rounded-lg tw:py-0 tw:text-[inherit] tw:shadow-none',
        PAD[padding],
        selected &&
          'tw:border-(--q-color-border-strong) tw:bg-(--q-color-surface-hover)',
        interactive && 'tw:cursor-pointer',
        className,
      )}
      style={{
        transition: `background ${t.motion.fast} ${t.motion.easeOut}, border-color ${t.motion.fast} ${t.motion.easeOut}`,
        ...style,
      }}
    >
      {children}
    </ShadcnCard>
  );
}
