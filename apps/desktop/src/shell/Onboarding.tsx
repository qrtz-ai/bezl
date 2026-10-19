import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  Button,
  Callout,
  OnboardingFrame,
  PermissionSetupList,
  PermissionSetupRow,
  Stack,
  Text,
  type IconName,
  type PermissionSetupState,
} from '@qrtz/ui';
import {
  loadConfig,
  type ModuleId,
  type OsPermission,
  type PermissionsStatus,
  type SettingsPane,
} from '@qrtz/core';
import { QrtzClientProvider, useQrtz, useQrtzEvent } from '@qrtz/core/react';
import { shellClient } from './shellClient';

/**
 * ADR 0184: the permissions qrtz can read and ask for up front, in the order the onboarding lists them. A row with
 * `only` is listed only while that app is installed (ADR 0184 amendment: system audio only with nano installed;
 * nano installed later asks for it the first time it records, or here when the onboarding is reopened).
 */
export const ONBOARDING_PERMISSIONS: readonly {
  id: OsPermission;
  pane: SettingsPane;
  icon: IconName;
  label: string;
  reason: string;
  only?: ModuleId;
}[] = [
  {
    id: 'microphone',
    pane: 'microphone',
    icon: 'mic',
    label: 'microphone',
    reason:
      'echo hears you while you hold the key; nano records your side of a meeting',
  },
  {
    id: 'screen-recording',
    pane: 'screen-recording',
    icon: 'screen-recording',
    label: 'system audio',
    reason: 'nano records the other side of a call',
    only: 'nano',
  },
  {
    id: 'accessibility',
    pane: 'accessibility',
    icon: 'accessibility',
    label: 'accessibility',
    reason:
      'echo types where your cursor is; snap and ctrl move windows and click for you',
  },
];

/** A grant a running qrtz only sees after a relaunch (macOS reads screen recording once per process). */
const RESTART_AFTER_GRANT: readonly OsPermission[] = ['screen-recording'];
/** How long qrtz keeps re-reading after an ask (then the window-focus re-read takes over). */
const WATCH_MS = 120_000;

/**
 * The rows to list for this config (the host's `modules.<id>.installed`, ADR 0169). Before config is read
 * (`null`), only the rows every install needs: a nano-only row never flashes in and out.
 */
export function onboardingPermissions(cfg: unknown) {
  const modules = cfg == null ? null : loadConfig(cfg).modules;
  return ONBOARDING_PERMISSIONS.filter(
    (d) => !d.only || (modules?.[d.only].installed ?? false),
  );
}

/** One row's state from the live status and whether the user has asked for it yet. */
export function setupState(
  st: string | undefined,
  id: OsPermission,
  asked: boolean,
): PermissionSetupState {
  if (st === 'granted') return 'granted';
  if (st === 'unsupported') return 'not-needed';
  if (st === 'denied' || st === 'not-determined')
    return asked && RESTART_AFTER_GRANT.includes(id)
      ? 'needs-restart'
      : 'not-granted';
  return 'checking';
}

/**
 * The first-run permissions onboarding (ADR 0184; shell-owned, sub apps unmounted). It never downloads anything:
 * models stay in each app's detail on the qrtz page. Continue and skip both finish it (`onboarding:finish`, the
 * host writes `onboarding.done`); a permission left off is asked the first time an app needs it.
 */
export function Onboarding({ onDone }: { onDone: () => void }) {
  const client = useMemo(shellClient, []);
  return (
    <QrtzClientProvider client={client}>
      <Steps onDone={onDone} />
    </QrtzClientProvider>
  );
}

function Steps({ onDone }: { onDone: () => void }) {
  const qrtz = useQrtz();
  const [perms, setPerms] = useState<PermissionsStatus | null>(null);
  const [asked, setAsked] = useState<{ p: OsPermission[]; at: number }>({
    p: [],
    at: 0,
  });
  const [error, setError] = useState<string | null>(null);
  const [finishing, setFinishing] = useState(false);
  const live = useQrtzEvent('permissions:changed');
  // Which apps are installed decides the rows (system audio only with nano), live as the host syncs qrtz.lock.
  const [cfg, setCfg] = useState<Record<string, unknown> | null>(null);
  const liveCfg = useQrtzEvent('config:changed');
  const fail = (e: Error) => setError(e.message);

  const read = useCallback(
    () => void qrtz.invoke('permissions:get').then(setPerms, fail),
    [qrtz],
  );
  useEffect(() => {
    read();
    // Granted in System Settings while qrtz was in the background: re-read when the window regains focus.
    window.addEventListener('focus', read);
    return () => window.removeEventListener('focus', read);
  }, [read]);
  useEffect(() => {
    if (live) setPerms(live);
  }, [live]);
  useEffect(() => {
    void qrtz.invoke('config:get').then(setCfg, fail);
  }, [qrtz]);
  useEffect(() => {
    if (liveCfg) setCfg(liveCfg);
  }, [liveCfg]);
  const shown = onboardingPermissions(cfg);
  // After an ask, re-read every second until the grant shows (System Settings may still be in front).
  const watching = asked.p.some(
    (p) => shown.some((d) => d.id === p) && perms?.[p] !== 'granted',
  );
  useEffect(() => {
    if (!watching) return;
    const id = setInterval(() => {
      if (Date.now() - asked.at > WATCH_MS) clearInterval(id);
      else read();
    }, 1000);
    return () => clearInterval(id);
  }, [watching, asked.at, read]);

  const markAsked = (p: OsPermission) =>
    setAsked((a) => ({ p: [...new Set([...a.p, p])], at: Date.now() }));
  const allow = (p: OsPermission) => {
    setError(null);
    markAsked(p);
    void qrtz
      .invoke('permissions:request', { permission: p })
      .then(setPerms, fail);
  };
  const open = (p: OsPermission, pane: SettingsPane) => {
    setError(null);
    markAsked(p);
    void qrtz
      .invoke('permissions:open-settings', { pane })
      .then(() => read(), fail);
  };
  const restart = () =>
    void qrtz.invoke('app:restart').then(() => undefined, fail);
  const finish = () => {
    setError(null);
    setFinishing(true);
    void qrtz.invoke('onboarding:finish').then(onDone, (e: Error) => {
      setFinishing(false);
      fail(e);
    });
  };

  const rows = shown.map((d) => ({
    ...d,
    state: setupState(perms?.[d.id], d.id, asked.p.includes(d.id)),
  }));
  const allGranted = rows.every(
    (r) => r.state === 'granted' || r.state === 'not-needed',
  );

  return (
    <OnboardingFrame
      title="set up qrtz"
      data-testid="onboarding"
      actions={
        <Button
          variant="ghost"
          size="sm"
          onClick={finish}
          disabled={finishing}
          data-testid="onboarding-skip"
        >
          skip
        </Button>
      }
    >
      <Text muted data-testid="onboarding-intro">
        {shown.some((d) => d.id === 'screen-recording')
          ? 'qrtz needs a few macos permissions to hear you, record calls and type for you.'
          : 'qrtz needs a few macos permissions to hear you and type for you.'}{' '}
        allow them now, or skip: each one is asked again the first time an app
        needs it.
      </Text>
      {error ? (
        <Callout tone="danger" title={error} data-testid="onboarding-error" />
      ) : null}
      <PermissionSetupList aria-label="permissions">
        {rows.map((r) => (
          <PermissionSetupRow
            key={r.id}
            id={r.id}
            icon={r.icon}
            label={r.label}
            reason={r.reason}
            state={r.state}
            {...(r.state === 'not-granted'
              ? { onAllow: () => allow(r.id) }
              : {})}
            {...(r.state === 'not-granted' || r.state === 'needs-restart'
              ? { onOpenSettings: () => open(r.id, r.pane) }
              : {})}
            {...(r.state === 'needs-restart' ? { onRestart: restart } : {})}
          />
        ))}
      </PermissionSetupList>
      <Stack dir="row" justify="end" gap={2}>
        <Button
          onClick={finish}
          disabled={finishing}
          data-testid="onboarding-continue"
        >
          {allGranted ? 'open qrtz' : 'continue'}
        </Button>
      </Stack>
    </OnboardingFrame>
  );
}
