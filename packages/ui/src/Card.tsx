import type { CSSProperties, HTMLAttributes, ReactNode } from 'react';
import { useTheme } from './theme';

/** qrtz.ai card: 1px border, rounded-lg, generous padding, no shadow. */
export function Card({
  children,
  padding = 6,
  interactive,
  selected,
  style,
  ...rest
}: {
  children: ReactNode;
  padding?: 3 | 4 | 6;
  interactive?: boolean;
  selected?: boolean;
  style?: CSSProperties;
} & HTMLAttributes<HTMLDivElement>) {
  const t = useTheme();
  return (
    <div
      {...rest}
      style={{
        border: `1px solid ${selected ? t.color.borderStrong : t.color.border}`,
        borderRadius: t.radius.lg,
        padding: t.space[padding],
        background: selected ? t.color.surfaceHover : t.color.card,
        cursor: interactive ? 'pointer' : undefined,
        transition: `background ${t.motion.fast} ${t.motion.easeOut}, border-color ${t.motion.fast} ${t.motion.easeOut}`,
        ...style,
      }}
    >
      {children}
    </div>
  );
}
