import { useTheme } from './theme';

export function Spinner({
  size = 16,
  label = 'Loading',
}: {
  size?: number;
  label?: string;
}) {
  const t = useTheme();
  return (
    <span
      role="status"
      aria-label={label}
      style={{
        display: 'inline-block',
        width: size,
        height: size,
        borderRadius: t.radius.full,
        border: `2px solid currentColor`,
        borderRightColor: 'transparent',
        animation: `q-spin 700ms linear infinite`,
        flexShrink: 0,
      }}
    />
  );
}

/**
 * A pane's loading state: a centered spinner filling the frame. The fallback a sub app shows while its view
 * suspends after its module loaded (a lazy screen, data), so a first visit never shows an empty frame.
 */
export function PaneLoading() {
  return (
    <div
      data-testid="pane-loading"
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        width: '100%',
        height: '100%',
        minHeight: 120,
        opacity: 0.6,
      }}
    >
      <Spinner size={18} label="loading" />
    </div>
  );
}
