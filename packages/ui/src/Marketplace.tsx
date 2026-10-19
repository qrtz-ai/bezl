import type { ReactNode } from 'react';
import { useTheme } from './theme';
import { Text } from './Text';
import { Badge, type BadgeTone } from './Badge';
import { Card } from './Card';
import { Icon, type IconName } from './Icon';
import { Progress } from './Progress';
import { Switch } from './Overlay';
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

export function ExtensionStatusBadge({ status }: { status: ExtensionStatus }) {
  const s = STATUS[status];
  return (
    <Badge tone={s.tone} data-testid="extension-status">
      {s.label}
    </Badge>
  );
}

/** The cards, in as many columns as fit. */
export function ExtensionGrid({
  children,
  'data-testid': testId,
}: {
  children: ReactNode;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <div
      role="list"
      data-testid={testId}
      style={{
        display: 'grid',
        gridTemplateColumns: 'repeat(auto-fill, minmax(min(220px, 100%), 1fr))',
        gap: t.space[4],
      }}
    >
      {children}
    </div>
  );
}

/** The square glyph at the top of a card and a detail page. */
export function ExtensionGlyph({
  icon,
  size = 'md',
}: {
  icon: IconName;
  size?: 'md' | 'lg';
}) {
  const t = useTheme();
  const px = size === 'lg' ? 56 : 40;
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
        border: `1px solid ${t.color.border}`,
        background: t.color.accent,
        color: t.color.text,
      }}
    >
      <Icon name={icon} size={size === 'lg' ? 'xl' : 'lg'} />
    </div>
  );
}

/** One extension in the marketplace: glyph, name, one line, size (with what it pulls in), and the button. */
export function ExtensionCard({
  name,
  icon,
  description,
  version,
  size,
  status,
  action,
  progress,
  onOpen,
}: {
  name: string;
  icon: IconName;
  description: string;
  version: string;
  /** Already formatted (`628 mb with dependencies`). */
  size: string;
  status: ExtensionStatus;
  /** The card's button (install / open / update), from the feature. */
  action?: ReactNode;
  /** 0..1 while installing. */
  progress?: number | null;
  onOpen: () => void;
}) {
  const t = useTheme();
  return (
    <div role="listitem" data-testid={`extension-card-${name}`}>
      <Card padding={4}>
        <div
          style={{ display: 'flex', flexDirection: 'column', gap: t.space[3] }}
        >
          <button
            type="button"
            onClick={onOpen}
            data-testid={`extension-open-${name}`}
            style={{
              all: 'unset',
              cursor: 'pointer',
              display: 'flex',
              flexDirection: 'column',
              gap: t.space[3],
            }}
          >
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
              }}
            >
              <ExtensionGlyph icon={icon} />
              <ExtensionStatusBadge status={status} />
            </div>
            <div style={{ display: 'grid', gap: 2 }}>
              <Text variant="h3">{lowerTitle(name)}</Text>
              <Text variant="caption" muted>
                {description.toLowerCase()}
              </Text>
            </div>
            <Text variant="caption" muted data-testid="extension-size">
              {`v${version} · ${size}`.toLowerCase()}
            </Text>
          </button>
          {typeof progress === 'number' ? (
            <Progress
              value={progress}
              label={`installing ${name}`}
              data-testid={`extension-progress-${name}`}
            />
          ) : null}
          {action ? <div style={{ display: 'flex' }}>{action}</div> : null}
        </div>
      </Card>
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
      <div style={{ flex: 1, minWidth: 0, display: 'grid', gap: 4 }}>
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
        minHeight: 28,
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
      style={{ margin: 0, padding: 0, display: 'grid', gap: 2 }}
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

/** One installed extension in the module manager: switch, name, meta, and its actions. */
export function ModuleRow({
  name,
  icon,
  meta,
  status,
  enabled,
  onToggle,
  toggleDisabled,
  actions,
  children,
}: {
  name: string;
  icon: IconName;
  meta: string;
  status: ExtensionStatus;
  enabled: boolean;
  onToggle: (on: boolean) => void;
  toggleDisabled?: boolean;
  actions?: ReactNode;
  /** Expanded details (dependencies). */
  children?: ReactNode;
}) {
  const t = useTheme();
  return (
    <div data-testid={`module-row-${name}`}>
      <Card padding={4}>
        <div
          style={{ display: 'flex', flexDirection: 'column', gap: t.space[3] }}
        >
          <div
            style={{ display: 'flex', alignItems: 'center', gap: t.space[3] }}
          >
            <ExtensionGlyph icon={icon} />
            <div style={{ flex: 1, minWidth: 0, display: 'grid', gap: 2 }}>
              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: t.space[2],
                }}
              >
                <Text variant="h3">{lowerTitle(name)}</Text>
                <ExtensionStatusBadge status={status} />
              </div>
              <Text variant="caption" muted truncate>
                {meta.toLowerCase()}
              </Text>
            </div>
            <Switch
              label={`${name} on`}
              checked={enabled}
              onChange={onToggle}
              disabled={toggleDisabled}
              data-testid={`module-enabled-${name}`}
            />
          </div>
          {actions ? (
            <div style={{ display: 'flex', flexWrap: 'wrap', gap: t.space[2] }}>
              {actions}
            </div>
          ) : null}
          {children}
        </div>
      </Card>
    </div>
  );
}
