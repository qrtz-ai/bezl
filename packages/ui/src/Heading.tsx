import type { CSSProperties, ReactNode } from 'react';
import { useTheme } from './theme';
import { typeStyle, type TypeVariant } from './tokens';
import { lowerTitle } from './case';

/**
 * Semantic heading: `level` picks the element (h1–h3) and its default style; `variant` overrides the look
 * (e.g. level={2} variant="h3"). `cursor` adds the qrtz.ai blinking underscore.
 */
export function Heading({
  children,
  level = 1,
  variant,
  cursor,
  style,
  preserveCase,
  ...rest
}: {
  /** Headings are lowercase (brand rule); set for user content such as a meeting title. */
  preserveCase?: boolean;
  children: ReactNode;
  level?: 1 | 2 | 3;
  variant?: Extract<
    TypeVariant,
    'display' | 'h1' | 'h2' | 'h3' | 'bodyLg' | 'label'
  >;
  cursor?: boolean;
  style?: CSSProperties;
  id?: string;
  'data-testid'?: string;
}) {
  const t = useTheme();
  const Tag = `h${level}` as const;
  return (
    <Tag
      {...rest}
      style={{
        margin: 0,
        ...typeStyle(variant ?? (`h${level}` as const), t),
        color: t.color.text,
        ...style,
      }}
    >
      {preserveCase ? children : lowerTitle(children)}
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
