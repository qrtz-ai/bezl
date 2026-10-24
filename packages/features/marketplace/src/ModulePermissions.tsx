import { useCallback, useEffect, useState } from 'react';
import { PermissionList } from '@qrtz/ui';
import {
  MODULE_PERMISSIONS,
  type ModuleId,
  type ModulePermission,
  type OsPermission,
  type PermissionsStatus,
  type SettingsPane,
} from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';
import { PERMISSIONS } from './catalog';

/** What each module uses a permission for (ADR 0163), shown beside its switch in the module manager. */
const WHY: Record<ModuleId, Partial<Record<ModulePermission, string>>> = {
  echo: {
    microphone: 'hears you while you hold the key',
    accessibility: 'types the text where your cursor is',
  },
  nano: {
    microphone: 'records your side of a meeting',
    'screen-recording': 'records the other side of the call',
    accessibility: 'reads who is talking in the meeting app',
  },
  ctrl: {
    accessibility: 'reads the window and clicks and types for a command',
  },
  snap: { accessibility: 'moves and sizes windows' },
};
/** The permissions qrtz can read and ask for up front; the others have no preflight and macOS asks on first use. */
const LIVE: readonly ModulePermission[] = [
  'microphone',
  'accessibility',
  'screen-recording',
];
const STATE: Record<string, string> = {
  granted: 'allowed',
  unsupported: 'not needed',
  unknown: 'checking…',
};

/** How long qrtz keeps watching for a switch after the user asked (then the window-focus re-read takes over). */
const WATCH_MS = 120_000;

/**
 * qrtz's live permission state (one identity for every module, ADR 0154). After "allow" it is re-read every second
 * until the grant shows up (qrtz-app reads accessibility and screen recording itself, live), so the row flips to
 * "allowed" while System Settings is still open; it is also re-read when the window regains focus.
 */
export function usePermissions() {
  const qrtz = useQrtz();
  const [perms, setPerms] = useState<PermissionsStatus | null>(null);
  const [asked, setAsked] = useState<{ p: OsPermission[]; at: number }>({
    p: [],
    at: 0,
  });
  const live = useQrtzEvent('permissions:changed');
  const read = useCallback(
    () => void qrtz.invoke('permissions:get').then(setPerms, () => {}),
    [qrtz],
  );
  useEffect(() => {
    read();
    // Granted in System Settings while qrtz was in the background.
    window.addEventListener('focus', read);
    return () => window.removeEventListener('focus', read);
  }, [read]);
  useEffect(() => {
    if (live) setPerms(live);
  }, [live]);
  const pending = asked.p.filter((p) => perms?.[p] !== 'granted');
  const watching = pending.length > 0;
  useEffect(() => {
    if (!watching) return;
    const id = setInterval(() => {
      if (Date.now() - asked.at > WATCH_MS) clearInterval(id);
      else read();
    }, 1000);
    return () => clearInterval(id);
  }, [watching, asked.at, read]);
  const ask = (p: OsPermission) => {
    setAsked((a) => ({ p: [...new Set([...a.p, p])], at: Date.now() }));
    return qrtz.invoke('permissions:request', { permission: p }).then(setPerms);
  };
  const open = (pane: SettingsPane) =>
    qrtz.invoke('permissions:open-settings', { pane }).then(() => undefined);
  const restart = () => qrtz.invoke('app:restart').then(() => undefined);
  return { perms, ask, open, restart, pending };
}

/**
 * ADR 0163 (no onboarding): a module's permissions with their state and "allow", where the user switches it on.
 * Nothing is asked until the user presses allow or first uses the feature.
 */
export function ModulePermissions({
  module,
  perms,
  onAsk,
  onOpen,
  onRestart,
  pending = [],
}: {
  module: ModuleId;
  perms: PermissionsStatus | null;
  onAsk: (p: OsPermission) => void;
  /** Opens a permission's System Settings pane (the ones macOS asks for on first use). */
  onOpen?: (p: SettingsPane) => void;
  onRestart?: () => void;
  /** Asked and not on yet: their rows say "switch on qrtz" while qrtz watches. */
  pending?: readonly OsPermission[];
}) {
  return (
    <PermissionList
      items={MODULE_PERMISSIONS[module].map((p) => {
        const why = WHY[module][p];
        const label = `${PERMISSIONS[p].label}${why ? `: ${why}` : ''}`;
        if (!LIVE.includes(p))
          return {
            id: `${module}-${p}`,
            label,
            state: 'asked on first use',
            ...(onOpen ? { onOpen: () => onOpen(p) } : {}),
          };
        const st = perms?.[p as OsPermission] ?? 'unknown';
        const askable = st === 'denied' || st === 'not-determined';
        const waiting = askable && pending.includes(p as OsPermission);
        return {
          id: `${module}-${p}`,
          label,
          state: STATE[st] ?? st,
          ...(askable ? { onGrant: () => onAsk(p as OsPermission) } : {}),
          ...(waiting ? { hint: 'switch on bezl' } : {}),
          // A screen recording grant reaches a running qrtz only after a relaunch.
          ...(waiting && p === 'screen-recording' && onRestart
            ? { onRestart }
            : {}),
        };
      })}
    />
  );
}
