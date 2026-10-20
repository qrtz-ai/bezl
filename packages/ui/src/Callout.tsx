import { lowerTitle } from './case';
import { typeStyle, tokens } from './tokens';
import type { ReactNode } from 'react';
import { useTheme } from './theme';

export type CalloutTone = 'info' | 'warning' | 'danger';

type Theme = ReturnType<typeof useTheme>;

/**
 * The alert outline every notice shares: a full 1px tone-tinted border on all sides, the tinted fill, radius md and
 * 10px 12px padding. Callout uses it, and so does the required-model banner, so every alert reads the same.
 */
export function calloutBox(t: Theme, tone: CalloutTone = 'info') {
  const c = {
    info: t.color.textMuted,
    warning: t.color.warning,
    danger: t.color.danger,
  }[tone];
  return {
    padding: `${tokens.space[2]} ${tokens.space[3]}`,
    border: `1px solid color-mix(in srgb, ${c} 35%, transparent)`,
    background: `color-mix(in srgb, ${c} 8%, transparent)`,
    borderRadius: t.radius.md,
  } as const;
}

/**
 * The one alert in qrtz (QRT-302): every notice, banner and "needs you" card is a Callout or a thin wrapper that
 * fills its slots. Danger/warning use role="alert" so screen readers announce them; info is a polite status.
 * `details` holds the technical reason, collapsed. `level="page"` marks the one page-level banner a screen may show.
 */
export function Callout({
  tone = 'info',
  title,
  children,
  action,
  details,
  level = 'inline',
  role,
  'aria-label': ariaLabel,
  'data-testid': testId,
}: {
  tone?: CalloutTone;
  title: ReactNode;
  children?: ReactNode;
  action?: ReactNode;
  /** Technical detail behind a "details" disclosure, collapsed by default. */
  details?: ReactNode;
  level?: 'inline' | 'page';
  /** Overrides the tone's role, e.g. "group" for a card that asks for a choice. */
  role?: string;
  'aria-label'?: string;
  'data-testid'?: string;
}) {
  const t = useTheme();
  const c = {
    info: t.color.textMuted,
    warning: t.color.warning,
    danger: t.color.danger,
  }[tone];
  return (
    <div
      role={role ?? (tone === 'info' ? 'status' : 'alert')}
      aria-label={ariaLabel}
      data-testid={testId}
      data-callout={tone}
      data-level={level}
      style={{
        display: 'flex',
        flexWrap: 'wrap',
        gap: tokens.space[3],
        alignItems: 'center',
        ...calloutBox(t, tone),
      }}
    >
      <span
        aria-hidden
        style={{
          width: 8,
          height: 8,
          borderRadius: tokens.radius.sm,
          background: c,
          flex: 'none',
        }}
      />
      <div style={{ flex: '1 1 160px', minWidth: 0 }}>
        <div
          style={{
            color: t.color.text,
            ...typeStyle('label', t),
          }}
        >
          {lowerTitle(title)}
        </div>
        {children ? (
          <div
            style={{
              color: t.color.textMuted,
              ...typeStyle('bodySm', t),
              marginTop: tokens.space[0.5],
              overflowWrap: 'anywhere',
            }}
          >
            {children}
          </div>
        ) : null}
        {details ? (
          <details
            data-testid={testId ? `${testId}-details` : undefined}
            style={{
              marginTop: tokens.space[1],
              color: t.color.textMuted,
              ...typeStyle('caption', t),
            }}
          >
            <summary style={{ cursor: 'pointer' }}>details</summary>
            <div
              style={{ marginTop: tokens.space[1], overflowWrap: 'anywhere' }}
            >
              {details}
            </div>
          </details>
        ) : null}
      </div>
      {action}
    </div>
  );
}
