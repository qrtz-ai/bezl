import * as SliderPrimitive from '@radix-ui/react-slider';
import { useState, type ReactNode } from 'react';
import { Icon } from './Icon';
import { lowerTitle } from './case';
import { Text } from './Text';
import { useTheme } from './theme';
import { typeStyle } from './tokens';

/**
 * A range slider on Radix Slider (shadcn `Slider`) with its value shown beside it. `onChange` follows the drag (live); `onCommit` fires once when the
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
      <SliderPrimitive.Root
        aria-label={label.toLowerCase()}
        min={min}
        max={max}
        step={step}
        value={[value]}
        onValueChange={([v]) => onChange?.(v!)}
        onValueCommit={([v]) => onCommit?.(v!)}
        style={{
          position: 'relative',
          flex: '1 1 auto',
          minWidth: 0,
          display: 'flex',
          alignItems: 'center',
          height: 18,
          cursor: 'pointer',
          touchAction: 'none',
          userSelect: 'none',
        }}
      >
        <SliderPrimitive.Track
          style={{
            position: 'relative',
            flexGrow: 1,
            height: 4,
            borderRadius: t.radius.full,
            background: t.color.borderStrong,
            overflow: 'hidden',
          }}
        >
          <SliderPrimitive.Range
            style={{
              position: 'absolute',
              height: '100%',
              background: t.color.primary,
            }}
          />
        </SliderPrimitive.Track>
        <SliderPrimitive.Thumb
          aria-label={label.toLowerCase()}
          data-testid={testId}
          style={{
            display: 'block',
            width: 14,
            height: 14,
            borderRadius: t.radius.full,
            background: t.color.primary,
            border: `2px solid ${t.color.bg}`,
            boxShadow: `0 0 0 1px ${t.color.borderStrong}`, // tokens-allow: thumb outline built from tokens
          }}
        />
      </SliderPrimitive.Root>
      <span
        style={{
          ...typeStyle('body', t),
          color: t.color.textMuted,
          minWidth: 52,
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
