import type { CSSProperties, ReactNode } from 'react';
import { useTheme } from './theme';

export interface TextProps {
  children: ReactNode;
  size?: 'sm' | 'md' | 'lg' | 'xl';
  muted?: boolean;
  weight?: 400 | 500 | 600;
  as?: 'span' | 'p' | 'h1' | 'h2';
  style?: CSSProperties;
}

export function Text({
  children,
  size = 'md',
  muted,
  weight = 400,
  as: Tag = 'span',
  style,
}: TextProps) {
  const t = useTheme();
  return (
    <Tag
      style={{
        margin: 0,
        fontSize: t.font.size[size],
        fontWeight: weight,
        color: muted ? t.color.textMuted : t.color.text,
        ...style,
      }}
    >
      {children}
    </Tag>
  );
}
