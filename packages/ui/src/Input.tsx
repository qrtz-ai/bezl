import * as SelectPrimitive from '@radix-ui/react-select';
import { Icon } from './Icon';
import { popoverSurface, itemRow } from './primitives/surface';
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

export const Input = forwardRef<
  HTMLInputElement,
  InputHTMLAttributes<HTMLInputElement>
>(function Input({ style, ...rest }, ref) {
  const base = useFieldStyle();
  return <input ref={ref} {...rest} style={{ ...base, ...style }} />;
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
 * A choice from a short fixed list on Radix Select (shadcn `Select`): a trigger that shows the value and a
 * popover listbox (ArrowUp/ArrowDown, type-ahead, Enter, Escape). Labels are lowercased (brand rule). Use the
 * native `Select` for long or dynamic lists.
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
  const base = useFieldStyle();
  return (
    <SelectPrimitive.Root
      value={value}
      onValueChange={(v) => onValueChange(v as T)}
      disabled={disabled}
    >
      <SelectPrimitive.Trigger
        aria-label={ariaLabel?.toLowerCase()}
        data-testid={testId}
        style={{
          ...base,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          gap: t.space[2],
          cursor: disabled ? 'not-allowed' : 'pointer',
          opacity: disabled ? 0.5 : 1,
          boxSizing: 'border-box',
        }}
      >
        <SelectPrimitive.Value />
        <SelectPrimitive.Icon asChild>
          <Icon name="chevron-down" size="sm" />
        </SelectPrimitive.Icon>
      </SelectPrimitive.Trigger>
      {/* Portalled (unlike Modal and Menu): an open Select hides everything outside its content from assistive
          tech, which would include its own focused trigger if rendered inline. Theme values are React context
          and inline styles, so they survive the portal. */}
      <SelectPrimitive.Portal>
        <SelectPrimitive.Content
          position="popper"
          sideOffset={4}
          data-q-no-ring
          data-testid={testId ? `${testId}-menu` : undefined}
          style={{
            ...popoverSurface(t),
            minWidth: 'var(--radix-select-trigger-width)',
          }}
        >
          <SelectPrimitive.Viewport>
            {options.map((o) => (
              <SelectPrimitive.Item
                key={o.value}
                value={o.value}
                data-q-item=""
                data-testid={testId ? `${testId}-${o.value}` : undefined}
                style={{ ...itemRow(t), justifyContent: 'space-between' }}
              >
                <SelectPrimitive.ItemText>
                  {lowerTitle(o.label)}
                </SelectPrimitive.ItemText>
                <SelectPrimitive.ItemIndicator asChild>
                  <Icon name="check" size="sm" />
                </SelectPrimitive.ItemIndicator>
              </SelectPrimitive.Item>
            ))}
          </SelectPrimitive.Viewport>
        </SelectPrimitive.Content>
      </SelectPrimitive.Portal>
    </SelectPrimitive.Root>
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
