import type { ReactNode } from 'react';
import { useTheme } from './theme';

export function EmptyState({
  title,
  body,
  action,
  icon,
}: {
  title: string;
  body?: string;
  action?: ReactNode;
  icon?: string;
}) {
  const t = useTheme();
  return (
    <div
      style={{
        display: 'grid',
        justifyItems: 'center',
        textAlign: 'center',
        gap: t.space[2],
        padding: t.space[12],
        color: t.color.textMuted,
      }}
    >
      {icon ? (
        <div style={{ fontSize: 28, fontFamily: t.font.mono }}>{icon}</div>
      ) : null}
      <div style={{ color: t.color.text, fontWeight: t.font.weight.semibold }}>
        {title}
      </div>
      {body ? (
        <div style={{ fontSize: t.font.size.sm, maxWidth: 360 }}>{body}</div>
      ) : null}
      {action}
    </div>
  );
}
