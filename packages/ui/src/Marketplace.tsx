import { useState, type CSSProperties, type ReactNode } from 'react';
import { useTheme } from './theme';
import { transition, tokens } from './tokens';
import { Text } from './Text';
import { Badge, type BadgeTone } from './Badge';
import { Icon, type IconName } from './Icon';
import { Progress } from './Progress';
import { lowerTitle } from './case';

/**
 * ADR 0161: the marketplace (an app-store browse of qrtz extensions) and the module manager. Data in, handlers
 * out; all copy lowercase. Feature code composes these; it never lays out cards itself.
 */

/** Where an extension stands for this user. */
export type ExtensionStatus =
  | 'available'
  | 'installed'
  | 'update'
  | 'built-in'
  | 'working';

const STATUS: Record<ExtensionStatus, { label: string; tone: BadgeTone }> = {
  available: { label: 'available', tone: 'neutral' },
  installed: { label: 'installed', tone: 'success' },
  update: { label: 'update', tone: 'warning' },
  'built-in': { label: 'built in', tone: 'neutral' },
  working: { label: 'working…', tone: 'primary' },
};

/** Healthy, expected states say nothing (QRT-303: badges only for exceptions); the row's action already implies them. */
const QUIET = new Set<ExtensionStatus>(['installed', 'built-in', 'available']);

export function ExtensionStatusBadge({ status }: { status: ExtensionStatus }) {
  if (QUIET.has(status)) return null;
  const s = STATUS[status];
  return (
    <Badge tone={s.tone} data-testid="extension-status">
      {s.label}
    </Badge>
  );
}

/** The square glyph at the top of a card and a detail page. */
export function ExtensionGlyph({
  icon,
  size = 'md',
}: {
  icon: IconName;
  size?: 'sm' | 'md' | 'lg';
}) {
  const t = useTheme();
  const px =
    tokens.size.appIcon[size === 'lg' ? 'lg' : size === 'sm' ? 'sm' : 'md'];
  return (
    <div
      aria-hidden
      style={{
        width: px,
        height: px,
        flexShrink: 0,
        display: 'grid',
        placeItems: 'center',
        borderRadius: t.radius.lg,
        border: `${tokens.border.hairline} solid ${t.color.border}`,
        background: t.color.accent,
        color: t.color.text,
      }}
    >
      <Icon
        name={icon}
        size={size === 'lg' ? 'xl' : size === 'sm' ? 'md' : 'lg'}
      />
    </div>
  );
}

/** A detail page's header: big glyph, name, publisher line, the main action. */
export function ExtensionHeader({
  name,
  icon,
  description,
  status,
  action,
}: {
  name: string;
  icon: IconName;
  description: string;
  status: ExtensionStatus;
  action?: ReactNode;
}) {
  const t = useTheme();
  return (
    <div
      data-testid="extension-header"
      style={{ display: 'flex', alignItems: 'center', gap: t.space[4] }}
    >
      <ExtensionGlyph icon={icon} size="lg" />
      <div
        style={{ flex: 1, minWidth: 0, display: 'grid', gap: tokens.space[1] }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: t.space[2] }}>
          <Text variant="h2">{lowerTitle(name)}</Text>
          <ExtensionStatusBadge status={status} />
        </div>
        <Text muted>{description.toLowerCase()}</Text>
      </div>
      {action}
    </div>
  );
}

/** Label / value rows (version, size, publisher, macos). */
export function FactList({
  items,
  'data-testid': testId,
}: {
  items: { label: string; value: ReactNode; 'data-testid'?: string }[];
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <dl
      data-testid={testId}
      style={{
        display: 'grid',
        gridTemplateColumns: 'minmax(0, max-content) minmax(0, 1fr)',
        columnGap: t.space[6],
        rowGap: t.space[2],
        margin: 0,
      }}
    >
      {items.map((i) => (
        <div key={i.label} style={{ display: 'contents' }}>
          <dt>
            <Text variant="caption" muted>
              {i.label.toLowerCase()}
            </Text>
          </dt>
          <dd
            style={{ margin: 0, minWidth: 0, overflowWrap: 'anywhere' }}
            data-testid={i['data-testid']}
          >
            <Text variant="caption">{i.value}</Text>
          </dd>
        </div>
      ))}
    </dl>
  );
}

/** One line with a leading Feather glyph: a permission, a dependency. */
export function IconLine({
  icon,
  title,
  meta,
  'data-testid': testId,
}: {
  icon: IconName;
  title: string;
  meta?: string;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <li
      data-testid={testId}
      style={{
        listStyle: 'none',
        display: 'flex',
        alignItems: 'center',
        gap: t.space[3],
        minHeight: tokens.size.row.sm,
      }}
    >
      <Icon name={icon} size="sm" tone="muted" />
      <Text>{title.toLowerCase()}</Text>
      {meta ? (
        <span style={{ marginLeft: 'auto' }}>
          <Text variant="caption" muted>
            {meta.toLowerCase()}
          </Text>
        </span>
      ) : null}
    </li>
  );
}

export function IconLines({
  children,
  'aria-label': label,
  'data-testid': testId,
}: {
  children: ReactNode;
  'aria-label'?: string;
  'data-testid'?: string;
}) {
  return (
    <ul
      aria-label={label}
      data-testid={testId}
      style={{ margin: 0, padding: 0, display: 'grid', gap: tokens.space[0.5] }}
    >
      {children}
    </ul>
  );
}

/** A determinate install bar with what it is doing (`downloading 314 of 628 mb`). */
export function InstallProgress({
  value,
  label,
  detail,
}: {
  value: number;
  label: string;
  detail: string;
}) {
  const t = useTheme();
  return (
    <div
      data-testid="install-progress"
      style={{ display: 'grid', gap: t.space[2] }}
    >
      <Progress value={value} label={label.toLowerCase()} />
      <Text variant="caption" muted role="status">
        {detail.toLowerCase()}
      </Text>
    </div>
  );
}

/** Read by screen readers, not shown (the AppRow button's trailing "details"). */
const VISUALLY_HIDDEN: CSSProperties = {
  position: 'absolute',
  width: tokens.size.srOnly,
  height: tokens.size.srOnly,
  margin: -tokens.size.srOnly,
  padding: 0,
  overflow: 'hidden',
  clip: 'rect(0 0 0 0)',
  clipPath: 'inset(50%)',
  whiteSpace: 'nowrap',
  border: 0,
};

/** The qrtz page's app lists ("installed", "available"): a plain list of AppRows, one per line. */
export function AppList({
  children,
  'aria-label': label,
  'data-testid': testId,
}: {
  children: ReactNode;
  'aria-label'?: string;
  'data-testid'?: string;
}) {
  return (
    <ul
      aria-label={label}
      data-testid={testId}
      data-app-list=""
      style={{
        margin: 0,
        padding: 0,
        display: 'grid',
        gap: tokens.space[0.5],
        minWidth: 0,
      }}
    >
      {children}
    </ul>
  );
}

/**
 * One app on the qrtz page, in a short row: a bare Feather icon (no tile, same size, tone and left edge as the
 * shared section's IconLine), name, one line of description, the version and size, an
 * optional status word, and its action on the right (a download icon, an inline progress bar, or the "more
 * actions" menu). Clicking anywhere on the row but the action opens the app's detail (`onOpen`); the action's
 * clicks and keys never reach the row.
 */
export function AppRow({
  name,
  icon,
  description,
  meta,
  status,
  statusTone = 'muted',
  action,
  onOpen,
}: {
  name: string;
  icon: IconName;
  description: string;
  /** Already formatted (`v0.3.0 · 628 mb with dependencies`, `v0.2.0 · built in`). */
  meta: string;
  /** A short word beside the action: built in, update, off, restart to update, couldn't install. */
  status?: string | null;
  statusTone?: 'muted' | 'warning' | 'danger' | 'success';
  action?: ReactNode;
  onOpen: () => void;
}) {
  const t = useTheme();
  const [hover, setHover] = useState(false);
  return (
    <li
      data-testid={`app-row-${name}`}
      data-app-row=""
      style={{ listStyle: 'none', minWidth: 0 }}
    >
      <div
        onClick={onOpen}
        onMouseEnter={() => setHover(true)}
        onMouseLeave={() => setHover(false)}
        style={{
          display: 'grid',
          gridTemplateColumns: 'minmax(0, 1fr) auto auto',
          alignItems: 'center',
          columnGap: t.space[3],
          // The row's own inset is pulled back out (negative margin), so its icon sits on the section label's
          // left edge and its action on the column's right edge, like the shared rows' IconLine; the hover fill
          // bleeds into the page gutter.
          padding: `${t.space[1]} ${t.space[2]}`,
          margin: `0 -${t.space[2]}`,
          borderRadius: t.radius.md,
          cursor: 'pointer',
          background: hover ? t.color.surfaceHover : 'transparent',
          transition: transition('background', 'hover', t),
          minWidth: 0,
        }}
      >
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onOpen();
          }}
          // WCAG 2.5.3 (axe label-content-name-mismatch): no aria-label. The button is named by what it shows
          // (name, meta, description) plus a visually hidden "details", so the name contains the visible text.
          data-testid={`app-open-${name}`}
          style={{
            all: 'unset',
            position: 'relative',
            boxSizing: 'border-box',
            display: 'grid',
            gridTemplateColumns: 'auto minmax(0, 1fr)',
            alignItems: 'center',
            columnGap: t.space[3],
            minWidth: 0,
            borderRadius: t.radius.sm,
          }}
        >
          <Icon
            name={icon}
            size="sm"
            tone="muted"
            data-testid={`app-icon-${name}`}
          />
          <span style={{ display: 'grid', gap: 0, minWidth: 0 }}>
            <span
              style={{
                display: 'flex',
                alignItems: 'baseline',
                gap: t.space[2],
                minWidth: 0,
              }}
            >
              <Text truncate>{lowerTitle(name)}</Text>
              <Text
                variant="caption"
                muted
                truncate
                data-testid={`app-meta-${name}`}
              >
                {meta.toLowerCase()}
              </Text>
            </span>
            <Text variant="caption" muted truncate>
              {description.toLowerCase()}
            </Text>
          </span>
          <span style={VISUALLY_HIDDEN}> details</span>
        </button>
        {status ? (
          <Text
            variant="caption"
            tone={statusTone}
            data-testid={`app-status-${name}`}
          >
            {status.toLowerCase()}
          </Text>
        ) : (
          <span />
        )}
        <span
          onClick={(e) => e.stopPropagation()}
          onKeyDown={(e) => e.stopPropagation()}
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'flex-end',
            minWidth: tokens.size.iconButton,
          }}
        >
          {action}
        </span>
      </div>
    </li>
  );
}

/**
 * A compact progress bar in a row's action slot, in place of its icon while an app downloads (its package and its
 * models, one bar). The percentage is its tooltip and its accessible value.
 */
export function RowProgress({
  value,
  label,
  'data-testid': testId,
}: {
  value: number;
  label: string;
  'data-testid'?: string;
}) {
  const pct = Math.round(Math.max(0, Math.min(1, value)) * 100);
  return (
    <span
      title={`${pct}%`}
      style={{ display: 'block', width: tokens.size.rowProgress }}
      data-testid={testId ? `${testId}-wrap` : undefined}
    >
      <Progress
        value={value}
        label={`${label.toLowerCase()} · ${pct}%`}
        data-testid={testId}
      />
    </span>
  );
}
