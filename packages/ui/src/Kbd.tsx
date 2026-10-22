import { typeStyle, tokens } from './tokens';
import type { ReactNode } from 'react';
import { useTheme } from './theme';

export function Kbd({ children }: { children: ReactNode }) {
  const t = useTheme();
  return (
    <kbd
      style={{
        ...typeStyle('mono', t),
        padding: `0 ${tokens.space[1]}`,
        borderRadius: t.radius.sm,
        border: `${tokens.border.hairline} solid ${t.color.border}`,
        borderBottomWidth: tokens.border.thick,
        background: t.color.surface,
        color: t.color.textMuted,
      }}
    >
      {children}
    </kbd>
  );
}
