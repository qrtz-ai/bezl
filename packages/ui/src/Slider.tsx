import { useState, type ReactNode } from 'react';
import { Icon } from './Icon';
import { lowerTitle } from './case';
import { Text } from './Text';
import { useTheme } from './theme';
import { typeStyle, tokens } from './tokens';

/**
 * A range slider with its value shown beside it. `onChange` follows the drag (live); `onCommit` fires once when the
 * drag or key press ends, for callers that persist (a config write per pixel would be wasteful).
 */
export function Slider({
  value,
  min,
  max,
  step = 0.01,
  label,
  unit = '',
  onChange,
  onCommit,
  'data-testid': testId,
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  /** Accessible name. */
  label: string;
  unit?: string;
  onChange?: (v: number) => void;
  onCommit?: (v: number) => void;
  'data-testid'?: string;
}) {
  const t = useTheme();
  const decimals = Math.max(0, Math.min(3, -Math.floor(Math.log10(step))));
  const read = (e: { currentTarget: HTMLInputElement }) =>
    Number(e.currentTarget.value);
  return (
    <span
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: t.space[2],
        width: '100%',
        minWidth: 0,
      }}
    >
      <input
        type="range"
        aria-label={label.toLowerCase()}
        data-testid={testId}
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange?.(read(e))}
        onPointerUp={(e) => onCommit?.(read(e))}
        onKeyUp={(e) => onCommit?.(read(e))}
        style={{
          flex: '1 1 auto',
          minWidth: 0,
          accentColor: t.color.primary,
          cursor: 'pointer',
        }}
      />
      <span
        style={{
          ...typeStyle('body', t),
          color: t.color.textMuted,
          minWidth: tokens.size.sliderValue,
          textAlign: 'right',
          fontVariantNumeric: 'tabular-nums',
        }}
      >
        {value.toFixed(decimals)}
        {unit ? ` ${unit}` : ''}
      </span>
    </span>
  );
}

/** A collapsed-by-default section: a lowercase header row that opens and closes its children. */
export function Disclosure({
  title,
  children,
  defaultOpen = false,
  'data-testid': testId,
}: {
  title: string;
  children: ReactNode;
  defaultOpen?: boolean;
  'data-testid'?: string;
}) {
  const t = useTheme();
  const [open, setOpen] = useState(defaultOpen);
  return (
    <section
      data-testid={testId}
      style={{ display: 'flex', flexDirection: 'column', gap: t.space[3] }}
    >
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        data-testid={testId ? `${testId}-toggle` : undefined}
        style={{
          all: 'unset',
          cursor: 'pointer',
          display: 'flex',
          alignItems: 'center',
          gap: t.space[2],
          color: t.color.textMuted,
        }}
      >
        <Text variant="label" tone="muted">
          {lowerTitle(title)}
        </Text>
        <Icon name={open ? 'chevron-down' : 'chevron-right'} size="sm" />
      </button>
      {open ? children : null}
    </section>
  );
}
