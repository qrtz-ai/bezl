import { Input as ShadcnInput } from './components/ui/input';
import { cn } from './lib/utils';
import {
  Select as ShadcnSelect,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from './components/ui/select';
import { Children, cloneElement, isValidElement, type ReactNode } from 'react';
import { lowerTitle } from './case';
import { Text } from './Text';
import { typeStyle } from './tokens';
import {
  forwardRef,
  type InputHTMLAttributes,
  type SelectHTMLAttributes,
} from 'react';
import { useTheme } from './theme';

const useFieldStyle = () => {
  const t = useTheme();
  return {
    height: 34,
    width: '100%',
    padding: `0 ${t.space[3]}`,
    borderRadius: t.radius.md,
    border: `1px solid ${t.color.border}`,
    background: t.color.bg,
    color: t.color.text,
    ...typeStyle('body', t),
  } as const;
};

/**
 * A text field on shadcn's Input with the qrtz field geometry (34px, body type, bg background) and the one global
 * focus ring (shadcn's ring is off). Text selection keeps the qrtz selection color.
 */
export const Input = forwardRef<
  HTMLInputElement,
  InputHTMLAttributes<HTMLInputElement>
>(function Input({ style, className, ...rest }, ref) {
  const t = useTheme();
  return (
    <ShadcnInput
      ref={ref}
      {...rest}
      className={cn(
        'tw:h-[34px] tw:border-border tw:bg-background tw:px-3 tw:py-0 tw:text-foreground tw:shadow-none tw:focus-visible:ring-0 tw:dark:bg-background tw:selection:bg-(--q-color-selection) tw:selection:text-foreground',
        className,
      )}
      style={{ ...typeStyle('body', t), ...style }}
    />
  );
});

/** Dropdown options are UI chrome: lowercase (brand rule), including brand and model names. */
const lowerOptions = (children: ReactNode): ReactNode =>
  Children.map(children, (c) =>
    isValidElement<{ children?: ReactNode }>(c) && c.type === 'option'
      ? cloneElement(c, {}, lowerTitle(c.props.children))
      : c,
  );

export function Select({
  style,
  children,
  ...rest
}: SelectHTMLAttributes<HTMLSelectElement>) {
  const base = useFieldStyle();
  return (
    <select {...rest} style={{ ...base, ...style }}>
      {lowerOptions(children)}
    </select>
  );
}

/**
 * A choice from a short fixed list on shadcn's Select: a trigger that shows the value and a popover listbox
 * (ArrowUp/ArrowDown, type-ahead, Enter, Escape). Labels are lowercased (brand rule). Use the native `Select`
 * for long or dynamic lists. The content is portalled (shadcn's default): an open Select aria-hides everything
 * outside it, which inline would include its own focused trigger.
 */
export function SelectMenu<T extends string>({
  value,
  onValueChange,
  options,
  disabled,
  'aria-label': ariaLabel,
  'data-testid': testId,
}: {
  value: T;
  onValueChange: (v: T) => void;
  options: { value: T; label: string }[];
  disabled?: boolean;
  'aria-label'?: string;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <ShadcnSelect
      value={value}
      onValueChange={(v) => onValueChange(v as T)}
      disabled={disabled}
    >
      <SelectTrigger
        aria-label={ariaLabel?.toLowerCase()}
        data-testid={testId}
        className="tw:h-[34px]! tw:w-full tw:cursor-pointer tw:bg-background tw:px-3 tw:py-0 tw:text-foreground tw:shadow-none tw:dark:bg-background tw:dark:hover:bg-background"
        style={typeStyle('body', t)}
      >
        <SelectValue />
      </SelectTrigger>
      <SelectContent
        position="popper"
        data-q-no-ring
        data-testid={testId ? `${testId}-menu` : undefined}
        className="tw:min-w-(--radix-select-trigger-width) tw:border-border-strong"
      >
        {options.map((o) => (
          <SelectItem
            key={o.value}
            value={o.value}
            data-testid={testId ? `${testId}-${o.value}` : undefined}
            className="tw:h-7 tw:cursor-pointer tw:py-0"
            style={typeStyle('bodySm', t)}
          >
            {lowerTitle(o.label)}
          </SelectItem>
        ))}
      </SelectContent>
    </ShadcnSelect>
  );
}

/** Labelled control. No hint text: the label and placeholder carry the meaning. */
export function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  const t = useTheme();
  return (
    <label style={{ display: 'grid', gap: t.space[2], minWidth: 0 }}>
      <Text>{lowerTitle(label)}</Text>
      {children}
    </label>
  );
}
