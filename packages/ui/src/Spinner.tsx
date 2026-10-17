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
