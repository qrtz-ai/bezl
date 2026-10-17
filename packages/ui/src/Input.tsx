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
    fontFamily: 'inherit',
    fontSize: t.font.size.md,
  } as const;
};

export const Input = forwardRef<
  HTMLInputElement,
  InputHTMLAttributes<HTMLInputElement>
>(function Input({ style, ...rest }, ref) {
  const base = useFieldStyle();
  return <input ref={ref} {...rest} style={{ ...base, ...style }} />;
});

export function Select({
  style,
  children,
  ...rest
}: SelectHTMLAttributes<HTMLSelectElement>) {
  const base = useFieldStyle();
  return (
    <select {...rest} style={{ ...base, ...style }}>
      {children}
    </select>
  );
}

export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  const t = useTheme();
  return (
    <label style={{ display: 'grid', gap: t.space[1] }}>
      <span
        style={{ fontSize: t.font.size.sm, fontWeight: t.font.weight.medium }}
      >
        {label}
      </span>
      {children}
      {hint ? (
        <span style={{ fontSize: t.font.size.xs, color: t.color.textMuted }}>
          {hint}
        </span>
      ) : null}
    </label>
  );
}
