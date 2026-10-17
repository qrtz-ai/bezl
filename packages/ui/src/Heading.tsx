import type { CSSProperties, ReactNode } from 'react';
import { useTheme } from './theme';

/** Display heading. `cursor` adds the qrtz.ai blinking underscore; `mono` for product names. */
export function Heading({
  children,
  level = 1,
  mono,
  cursor,
  style,
}: {
  children: ReactNode;
  level?: 1 | 2 | 3;
  mono?: boolean;
  cursor?: boolean;
  style?: CSSProperties;
}) {
  const t = useTheme();
  const Tag = `h${level}` as const;
  const size = { 1: t.font.size['2xl'], 2: t.font.size.xl, 3: t.font.size.lg }[
    level
  ];
  return (
    <Tag
      style={{
        margin: 0,
        fontSize: size,
        fontWeight: level === 1 ? t.font.weight.bold : t.font.weight.semibold,
        letterSpacing: '-0.02em',
        lineHeight: t.font.lineHeight.tight,
        fontFamily: mono ? t.font.mono : 'inherit',
        color: t.color.text,
        ...style,
      }}
    >
      {children}
      {cursor ? (
        <span
          aria-hidden
          style={{ marginLeft: 2, animation: 'q-blink 1.1s step-end infinite' }}
        >
          _
        </span>
      ) : null}
    </Tag>
  );
}
