import { Children, cloneElement, isValidElement, type ReactNode } from 'react';
import { lowerTitle } from './case';
import { Label } from './Text';
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
    <label style={{ display: 'grid', gap: t.space[1] }}>
      <Label>{lowerTitle(label)}</Label>
      {children}
    </label>
  );
}
