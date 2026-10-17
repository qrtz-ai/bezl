import { useTheme } from './theme';
import { transition } from './tokens';

/** Determinate progress bar (value 0..1). Accessible name via `label`. */
export function Progress({
  value,
  label,
  'data-testid': testId,
}: {
  value: number;
  label: string;
  'data-testid'?: string;
}) {
  const t = useTheme();
  const pct = Math.round(Math.max(0, Math.min(1, value)) * 100);
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={pct}
      data-testid={testId}
      style={{
        height: 6,
        borderRadius: t.radius.full,
        background: t.color.border,
        overflow: 'hidden',
      }}
    >
      <div
        style={{
          width: `${pct}%`,
          height: '100%',
          background: t.color.primary,
          transition: transition('width', 'hover', t),
        }}
      />
    </div>
  );
}
