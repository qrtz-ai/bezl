import type { ReactNode } from 'react';
import { useTheme } from './theme';
import { Text } from './Text';
import { Icon, type IconName } from './Icon';

/** Quiet centered placeholder: optional glyph, one short title, optional action. No body copy. */
export function EmptyState({
  title,
  action,
  icon,
}: {
  title: string;
  action?: ReactNode;
  icon?: IconName;
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
      {icon ? <Icon name={icon} size="xl" tone="muted" /> : null}
      <Text muted>{title.toLowerCase()}</Text>
      {action}
    </div>
  );
}
