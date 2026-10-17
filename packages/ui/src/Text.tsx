import type { CSSProperties, HTMLAttributes, ReactNode } from 'react';
import { useTheme } from './theme';

export interface TextProps extends Omit<HTMLAttributes<HTMLElement>, 'style'> {
  children?: ReactNode;
  size?: 'xs' | 'sm' | 'md' | 'lg' | 'xl';
  muted?: boolean;
  weight?: 400 | 500 | 600 | 700;
  mono?: boolean;
  as?: 'span' | 'p' | 'div' | 'h1' | 'h2' | 'h3';
  style?: CSSProperties;
}

export function Text({
  children,
  size = 'md',
  muted,
  weight = 400,
  mono,
  as: Tag = 'span',
  style,
  ...rest
}: TextProps) {
  const t = useTheme();
  return (
    <Tag
      {...rest}
      style={{
        margin: 0,
        fontSize: t.font.size[size],
        fontWeight: weight,
        fontFamily: mono ? t.font.mono : undefined,
        color: muted ? t.color.textMuted : t.color.text,
        ...style,
      }}
    >
      {children}
    </Tag>
  );
}
