import type {
  AnchorHTMLAttributes,
  CSSProperties,
  HTMLAttributes,
  LabelHTMLAttributes,
  ReactNode,
} from 'react';
import { useTheme } from './theme';
import {
  transition,
  typeStyle,
  type Tokens,
  type TypeVariant,
  tokens,
} from './tokens';

export type TextTone =
  | 'default'
  | 'muted'
  | 'subtle'
  | 'danger'
  | 'warning'
  | 'success'
  | 'inherit';

const toneColor = (t: Tokens, tone: TextTone) =>
  ({
    default: t.color.text,
    muted: t.color.textMuted,
    subtle: t.color.textSubtle,
    danger: t.color.danger,
    warning: t.color.warning,
    success: t.color.success,
    inherit: 'inherit',
  })[tone];

export interface TextProps extends Omit<HTMLAttributes<HTMLElement>, 'style'> {
  children?: ReactNode;
  /** A named style from the type scale (default `body`). */
  variant?: TypeVariant;
  tone?: TextTone;
  /** Shorthand for tone="muted". */
  muted?: boolean;
  /** Single line with ellipsis. */
  truncate?: boolean;
  as?: 'span' | 'p' | 'div' | 'strong' | 'em' | 'time';
  style?: CSSProperties;
}

/** The only way to set text outside @qrtz/ui: pick a `variant` from the type scale. */
export function Text({
  children,
  variant = 'body',
  tone,
  muted,
  truncate,
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
        ...typeStyle(variant, t),
        color: toneColor(t, tone ?? (muted ? 'muted' : 'default')),
        ...(truncate
          ? {
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
              minWidth: 0,
            }
          : // An unbroken word (a path, a token) wraps instead of pushing its container sideways (QRT-304).
            { overflowWrap: 'anywhere' }),
        ...style,
      }}
    >
      {children}
    </Tag>
  );
}

/** Form/section label. Renders a <label> when `htmlFor` is given; `caps` for small uppercase eyebrows. */
export function Label({
  children,
  htmlFor,
  tone = 'default',
  caps,
  style,
  ...rest
}: Omit<LabelHTMLAttributes<HTMLLabelElement>, 'style'> & {
  tone?: TextTone;
  caps?: boolean;
  style?: CSSProperties;
}) {
  const t = useTheme();
  const Tag = htmlFor ? 'label' : 'span';
  return (
    <Tag
      {...rest}
      {...(htmlFor ? { htmlFor } : {})}
      style={{
        margin: 0,
        ...typeStyle('label', t),
        color: toneColor(t, tone),
        ...(caps ? { textTransform: 'lowercase' as const } : null),
        ...style,
      }}
    >
      {children}
    </Tag>
  );
}

/** Inline code chip, or with `block` a preformatted block. */
export function Code({
  children,
  block,
  style,
  ...rest
}: Omit<HTMLAttributes<HTMLElement>, 'style'> & {
  children: ReactNode;
  block?: boolean;
  style?: CSSProperties;
}) {
  const t = useTheme();
  const base: CSSProperties = {
    ...typeStyle('code', t),
    color: t.color.text,
    background: t.color.surfaceHover,
    border: `1px solid ${t.color.border}`,
    borderRadius: t.radius.sm,
  };
  return block ? (
    <pre
      {...rest}
      style={{
        ...base,
        margin: 0,
        padding: t.space[3],
        overflow: 'auto',
        maxWidth: '100%',
        minWidth: 0,
        whiteSpace: 'pre-wrap',
        overflowWrap: 'anywhere',
        ...style,
      }}
    >
      <code>{children}</code>
    </pre>
  ) : (
    <code
      {...rest}
      style={{
        ...base,
        padding: `0 ${tokens.space[1]}`,
        overflowWrap: 'anywhere',
        ...style,
      }}
    >
      {children}
    </code>
  );
}

/** Figures (timers, shortcuts, ids) in the mono style. */
export function Mono(props: Omit<TextProps, 'variant'>) {
  return <Text {...props} variant="mono" />;
}

/** Inline link: <a> with `href`, otherwise a button that looks like a link. */
export function Link({
  children,
  href,
  onClick,
  variant = 'body',
  tone = 'default',
  style,
  ...rest
}: Omit<AnchorHTMLAttributes<HTMLAnchorElement>, 'style' | 'onClick'> & {
  onClick?: () => void;
  variant?: TypeVariant;
  tone?: TextTone;
  style?: CSSProperties;
}) {
  const t = useTheme();
  const css: CSSProperties = {
    ...typeStyle(variant, t),
    color: toneColor(t, tone),
    textDecoration: 'underline',
    textUnderlineOffset: '3px',
    textDecorationThickness: '1px',
    cursor: 'pointer',
    borderRadius: t.radius.sm,
    transition: transition('color', 'hover', t),
    ...style,
  };
  if (href)
    return (
      <a {...rest} href={href} onClick={onClick} style={css}>
        {children}
      </a>
    );
  return (
    <button
      type="button"
      data-testid={(rest as { 'data-testid'?: string })['data-testid']}
      aria-label={rest['aria-label']}
      onClick={onClick}
      style={{ all: 'unset', ...css }}
    >
      {children}
    </button>
  );
}

/** A long preformatted text inside a dialog (settings → "open-source licenses"): it scrolls, at most 60% of the window. */
export function LongText({
  children,
  'data-testid': testId,
}: {
  children: ReactNode;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <div style={{ padding: t.space[4] }}>
      <Code
        block
        data-testid={testId}
        tabIndex={0}
        style={{ maxHeight: '60vh' }}
      >
        {children}
      </Code>
    </div>
  );
}
