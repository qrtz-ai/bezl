import type { ReactNode } from 'react';
import { useTheme } from './theme';

export type CalloutTone = 'info' | 'warning' | 'danger';

/**
 * Inline status/error banner with an optional action (e.g. Retry). Danger/warning use role="alert"
 * so screen readers announce them; info is a polite status.
 */
export function Callout({
  tone = 'info',
  title,
  children,
  action,
  'data-testid': testId,
}: {
  tone?: CalloutTone;
  title: ReactNode;
  children?: ReactNode;
  action?: ReactNode;
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
      role={tone === 'info' ? 'status' : 'alert'}
      data-testid={testId}
      style={{
        display: 'flex',
        gap: 12,
        alignItems: 'center',
        padding: '10px 12px',
        border: `1px solid color-mix(in srgb, ${c} 35%, transparent)`,
        background: `color-mix(in srgb, ${c} 8%, transparent)`,
        borderRadius: t.radius.md,
      }}
    >
      <span
        aria-hidden
        style={{
          width: 8,
          height: 8,
          borderRadius: 4,
          background: c,
          flex: 'none',
        }}
      />
      <div style={{ flex: 1, minWidth: 0 }}>
        <div
          style={{
            color: t.color.text,
            fontWeight: t.font.weight.semibold,
            fontSize: t.font.size.sm,
          }}
        >
          {title}
        </div>
        {children ? (
          <div
            style={{
              color: t.color.textMuted,
              fontSize: t.font.size.xs,
              marginTop: 2,
              overflowWrap: 'anywhere',
            }}
          >
            {children}
          </div>
        ) : null}
      </div>
      {action}
    </div>
  );
}
