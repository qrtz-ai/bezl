import type { ReactNode } from 'react';
import { Badge, type BadgeTone } from './Badge';
import { Button } from './Button';
import { Icon, type IconName } from './Icon';
import { Page } from './Layout';
import { Text } from './Text';
import { useTheme } from './theme';

/**
 * ADR 0184: the first-run permissions onboarding's window. It takes the whole window (no sidebar: it shows before
 * the qrtz page), keeps the window drag strip, and holds one shared-geometry `Page` (880px column, 40px header row,
 * 32px gutters, 40px top, 24px between sections). It scrolls vertically only.
 */
export function OnboardingFrame({
  title,
  actions,
  children,
  'data-testid': testId,
}: {
  title: string;
  /** Right of the header row (skip). */
  actions?: ReactNode;
  children: ReactNode;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <div
      style={{
        height: '100%',
        background: t.color.bg,
        overflow: 'hidden',
        display: 'grid',
      }}
    >
      <div className="q-drag" aria-hidden />
      <main
        data-scroll-root=""
        data-testid={testId}
        style={{
          minWidth: 0,
          minHeight: 0,
          overflowX: 'hidden',
          overflowY: 'auto',
          overscrollBehavior: 'none',
          scrollbarGutter: 'stable',
        }}
      >
        <Page title={title} actions={actions}>
          {children}
        </Page>
      </main>
    </div>
  );
}

/** What a permission row says about its grant. */
export type PermissionSetupState =
  | 'granted'
  | 'not-granted'
  | 'needs-restart'
  | 'checking'
  | 'not-needed';

const STATE: Record<PermissionSetupState, { text: string; tone: BadgeTone }> = {
  granted: { text: 'granted', tone: 'success' },
  'not-granted': { text: 'not granted', tone: 'warning' },
  'needs-restart': { text: 'needs restart', tone: 'warning' },
  checking: { text: 'checking…', tone: 'neutral' },
  'not-needed': { text: 'not needed', tone: 'neutral' },
};

/** The permission rows of the onboarding, one per line. */
export function PermissionSetupList({
  children,
  'aria-label': label,
}: {
  children: ReactNode;
  'aria-label'?: string;
}) {
  const t = useTheme();
  return (
    <ul
      aria-label={label}
      style={{
        listStyle: 'none',
        margin: 0,
        padding: 0,
        display: 'grid',
        gap: t.space[2],
        minWidth: 0,
      }}
    >
      {children}
    </ul>
  );
}

/**
 * One permission in the onboarding: its glyph, name and what qrtz uses it for, the live state, and its actions.
 * "allow" asks macOS (the real system prompt, or System Settings at the pane when macOS has no prompt for it);
 * "settings" opens the pane; "restart qrtz" is for a grant a running qrtz only sees after a relaunch. Each
 * button's accessible name contains its visible text plus the permission (WCAG 2.5.3).
 */
export function PermissionSetupRow({
  id,
  icon,
  label,
  reason,
  state,
  onAllow,
  onOpenSettings,
  onRestart,
}: {
  id: string;
  icon: IconName;
  label: string;
  reason: string;
  state: PermissionSetupState;
  onAllow?: () => void;
  onOpenSettings?: () => void;
  onRestart?: () => void;
}) {
  const t = useTheme();
  const s = STATE[state];
  const name = label.toLowerCase();
  return (
    <li
      data-testid={`onboarding-perm-${id}`}
      data-state={state}
      style={{
        display: 'grid',
        gridTemplateColumns: 'auto minmax(0, 1fr) auto',
        alignItems: 'center',
        columnGap: t.space[3],
        rowGap: t.space[2],
        minHeight: 52,
        padding: `${t.space[2]} ${t.space[3]}`,
        borderRadius: t.radius.md,
        border: `1px solid ${t.color.border}`,
        minWidth: 0,
      }}
    >
      <Icon name={icon} tone="muted" />
      <span style={{ display: 'grid', gap: 0, minWidth: 0 }}>
        <Text truncate>{name}</Text>
        <Text variant="caption" muted>
          {reason.toLowerCase()}
        </Text>
      </span>
      <span
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'flex-end',
          flexWrap: 'wrap',
          gap: t.space[2],
          minWidth: 0,
        }}
      >
        <Badge
          tone={s.tone}
          role="status"
          data-testid={`onboarding-perm-${id}-state`}
        >
          {s.text}
        </Badge>
        {onAllow ? (
          <Button
            size="sm"
            variant="primary"
            onClick={onAllow}
            aria-label={`allow ${name}`}
            data-testid={`onboarding-perm-${id}-allow`}
          >
            allow
          </Button>
        ) : null}
        {onOpenSettings ? (
          <Button
            size="sm"
            variant="ghost"
            icon="open"
            onClick={onOpenSettings}
            aria-label={`${name} settings`}
            data-testid={`onboarding-perm-${id}-open`}
          >
            settings
          </Button>
        ) : null}
        {onRestart ? (
          <Button
            size="sm"
            variant="secondary"
            icon="restart"
            onClick={onRestart}
            data-testid={`onboarding-perm-${id}-restart`}
          >
            restart qrtz
          </Button>
        ) : null}
      </span>
    </li>
  );
}
