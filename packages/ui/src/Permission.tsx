import type { ReactNode } from 'react';
import { Button } from './Button';
import { Callout } from './Callout';
import { Blocked } from './ModelRequirement';
import { Spinner } from './Spinner';
import { useTheme } from './theme';

/**
 * Every macOS permission qrtz needs is granted from inside qrtz (Jared, 6:18 PM): one button opens System Settings
 * at the exact pane (after qrtz has asked macOS, which adds qrtz to that list), one line says what to do there,
 * and the caller polls the grant while this shows, so it disappears on its own. `onRestart` adds "restart qrtz"
 * for a grant a running qrtz only sees after a relaunch.
 */
export function PermissionPrompt({
  title,
  pane,
  onOpen,
  waiting,
  onRestart,
  children,
  'data-testid': testId,
}: {
  title: ReactNode;
  /** The pane's name in the button: "open {pane} settings". */
  pane: string;
  onOpen: () => void;
  /** The user opened the pane and qrtz is watching for the switch. */
  waiting?: boolean;
  onRestart?: () => void;
  children?: ReactNode;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <Callout
      tone="warning"
      title={title}
      data-testid={testId}
      action={
        <div style={{ display: 'flex', gap: t.space[2], flex: 'none' }}>
          <Button
            size="sm"
            variant="primary"
            icon="open"
            onClick={onOpen}
            data-testid={testId ? `${testId}-open` : undefined}
          >
            {`open ${pane} settings`}
          </Button>
          {onRestart ? (
            <Button
              size="sm"
              variant="secondary"
              icon="restart"
              onClick={onRestart}
              data-testid={testId ? `${testId}-restart` : undefined}
            >
              restart qrtz
            </Button>
          ) : null}
        </div>
      }
    >
      <span
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          gap: t.space[2],
        }}
      >
        <span>switch on qrtz</span>
        {waiting ? (
          <span
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              gap: t.space[1],
            }}
            data-testid={testId ? `${testId}-waiting` : undefined}
          >
            <Spinner size={12} label="waiting" />
            waiting for the switch…
          </span>
        ) : null}
      </span>
      {children}
    </Callout>
  );
}

/**
 * ModelRequired's gate for a permission: until `granted`, the PermissionPrompt banner (its button opens the pane)
 * sits over the children, blurred and inert.
 */
export function PermissionRequired({
  granted,
  children,
  ...prompt
}: Parameters<typeof PermissionPrompt>[0] & { granted: boolean }) {
  const t = useTheme();
  if (granted) return <>{children}</>;
  const testId = prompt['data-testid'];
  return (
    <div
      style={{ display: 'grid', gap: t.space[6], minWidth: 0, width: '100%' }}
    >
      <PermissionPrompt {...prompt} />
      <Blocked data-testid={testId ? `${testId}-blocked` : undefined}>
        {children}
      </Blocked>
    </div>
  );
}
