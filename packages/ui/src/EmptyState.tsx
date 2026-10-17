import type { ReactNode } from 'react';
import { useTheme } from './theme';
import { Text } from './Text';

/** Quiet centered placeholder: optional glyph, one short title, optional action. No body copy. */
export function EmptyState({
  title,
  action,
  icon,
}: {
  title: string;
  action?: ReactNode;
  icon?: string;
}) {
  const t = useTheme();
  return (
    <div
      data-testid="empty-state"
      style={{
        display: 'grid',
        justifyItems: 'center',
        textAlign: 'center',
        gap: t.space[3],
        padding: t.space[12],
      }}
    >
      {icon ? (
        <Text variant="h2" muted aria-hidden>
          {icon}
        </Text>
      ) : null}
      <Text muted>{title}</Text>
      {action}
    </div>
  );
}
