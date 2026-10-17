import type { ReactNode } from 'react';
import { useTheme } from './theme';

export function Kbd({ children }: { children: ReactNode }) {
  const t = useTheme();
  return (
    <kbd
      style={{
        fontFamily: t.font.mono,
        fontSize: t.font.size.xs,
        padding: '1px 6px',
        borderRadius: t.radius.sm,
        border: `1px solid ${t.color.border}`,
        borderBottomWidth: 2,
        background: t.color.surface,
        color: t.color.textMuted,
      }}
    >
      {children}
    </kbd>
  );
}
