import { Button as ShadcnButton } from './components/ui/button';
import { cn } from './lib/utils';
import { lowerTitle } from './case';
import type { ButtonHTMLAttributes, CSSProperties } from 'react';
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

const VARIANT = {
  primary: 'default',
  secondary: 'secondary',
  ghost: 'ghost',
  danger: 'destructive',
} as const;
const SIZE = { sm: 'sm', md: 'default', lg: 'lg' } as const;

/**
 * The button: stock shadcn Button (variants primary = default, secondary, ghost, danger = destructive; sizes sm,
 * md = default, lg) on the qrtz tokens. Only qrtz behavior is added: lowercase copy, Feather icons, loading.
 */
export function Button({
  variant = 'secondary',
  size = 'md',
  loading,
  icon,
  block,
  disabled,
  style,
  className,
  children,
  ...rest
}: ButtonProps) {
  const off = disabled || loading;
  return (
    <ShadcnButton
      type="button"
      disabled={off}
      aria-busy={loading || undefined}
      {...rest}
      variant={VARIANT[variant]}
      size={SIZE[size]}
      // Our variant name, which primaryButtons() and the tests read (shadcn's would be "default").
      data-variant={variant}
      // Non-stock: long labels clip inside the container instead of spilling out (QRT-304).
      className={cn(
        'tw:max-w-full tw:overflow-hidden',
        // Ghost text is the foreground color, not the muted text it may sit in (contrast).
        variant === 'ghost' && 'tw:text-foreground',
        // Danger fills with dangerSolid (4.8:1 with white); --destructive stays the brighter danger text color.
        variant === 'danger' &&
          'tw:bg-(--q-color-danger-solid) tw:dark:bg-(--q-color-danger-solid) tw:hover:bg-(--q-color-danger-solid)',
        block && 'tw:w-full',
        className,
      )}
      style={{ WebkitAppRegion: 'no-drag', ...style } as CSSProperties}
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
