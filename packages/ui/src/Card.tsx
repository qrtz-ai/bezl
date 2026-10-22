import {
  Card as ShadcnCard,
  CardContent,
  CardFooter,
} from './components/ui/card';
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
 * One cell of a CardGrid: a stock shadcn Card that fills its cell, content above and the `footer` in a CardFooter
 * at the bottom, so buttons line up across a row whatever the text above them.
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
  return (
    <div
      role={role}
      data-testid={testId}
      data-grid-card=""
      style={{ display: 'flex', minWidth: 0, height: '100%' }}
    >
      <ShadcnCard className="tw:min-w-0 tw:flex-1">
        <CardContent className="tw:flex-1">{children}</CardContent>
        {footer ? (
          <CardFooter
            data-card-footer=""
            className="tw:flex-col tw:items-stretch tw:gap-2"
          >
            {footer}
          </CardFooter>
        ) : null}
      </ShadcnCard>
    </div>
  );
}

/**
 * qrtz.ai card: a stock shadcn Card (its padding, gap, radius, border and shadow) with the children in a
 * CardContent. `selected` swaps the border and background colors; `padding` is kept for API compatibility.
 */
export function Card({
  children,
  padding: _padding,
  interactive,
  selected,
  className,
  ...rest
}: {
  children: ReactNode;
  padding?: 3 | 4 | 6;
  interactive?: boolean;
  selected?: boolean;
  style?: CSSProperties;
} & HTMLAttributes<HTMLDivElement>) {
  return (
    <ShadcnCard
      {...rest}
      className={cn(
        selected && 'tw:border-border-strong tw:bg-muted',
        interactive && 'tw:cursor-pointer',
        className,
      )}
    >
      <CardContent>{children}</CardContent>
    </ShadcnCard>
  );
}
