import { Button as ShadcnButton } from './components/ui/button';
import { cn } from './lib/utils';
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

const VARIANT: Record<
  ButtonVariant,
  {
    shadcn: 'default' | 'secondary' | 'ghost' | 'destructive';
    className: string;
  }
> = {
  primary: { shadcn: 'default', className: 'tw:border-primary' },
  secondary: { shadcn: 'secondary', className: 'tw:border-border' },
  ghost: {
    shadcn: 'ghost',
    className: 'tw:border-border tw:bg-transparent tw:text-foreground',
  },
  // qrtz's solid danger (dangerSolid, 4.8:1 with its text) instead of shadcn's 60% destructive in dark.
  danger: {
    shadcn: 'destructive',
    className:
      'tw:border-(--q-color-danger-solid) tw:bg-(--q-color-danger-solid) tw:text-(--q-color-danger-solid-text) tw:dark:bg-(--q-color-danger-solid)',
  },
};
const SIZE: Record<ButtonSize, string> = {
  sm: 'tw:h-7 tw:px-3 tw:has-[>svg]:px-3',
  md: 'tw:h-[34px] tw:px-4 tw:has-[>svg]:px-4',
  lg: 'tw:h-10 tw:px-5 tw:has-[>svg]:px-5',
};

/**
 * The button, on shadcn's Button (cva variants: primary = default, secondary, ghost, danger = destructive) with
 * qrtz geometry: 28/34/40px, a 1px border on every variant, label type, Feather icons at 14px, and the global
 * focus ring (shadcn's ring is off so there is one ring). Disabled stays hoverable (its title still shows).
 */
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
  const off = disabled || loading;
  const v = VARIANT[variant];
  return (
    <ShadcnButton
      type="button"
      disabled={off}
      aria-busy={loading || undefined}
      {...rest}
      variant={v.shadcn}
      // Our variant name, which primaryButtons() and the tests read (shadcn's would be "default").
      data-variant={variant}
      className={cn(
        'tw:max-w-full tw:shrink tw:cursor-pointer tw:overflow-hidden tw:border tw:shadow-none tw:transition-[opacity,background] tw:focus-visible:ring-0 tw:disabled:pointer-events-auto tw:disabled:cursor-not-allowed tw:disabled:opacity-55 tw:[&_svg:not([class*=size-])]:size-3.5',
        v.className,
        SIZE[size],
        block && 'tw:box-border tw:w-full',
      )}
      style={
        {
          ...typeStyle('label', t),
          WebkitAppRegion: 'no-drag',
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
    </ShadcnButton>
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
