import { useEffect, useState } from 'react';
import { PermissionList } from '@qrtz/ui';
import {
  MODULE_PERMISSIONS,
  type ModuleId,
  type ModulePermission,
  type OsPermission,
  type PermissionsStatus,
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
    accessibility: 'clicks and types for a task',
    automation: 'asks apps to do a task',
    'files-and-folders': 'tidies the folders a task names',
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

/** qrtz's live permission state (one identity for every module, ADR 0154), re-read when the window regains focus. */
export function usePermissions() {
  const qrtz = useQrtz();
  const [perms, setPerms] = useState<PermissionsStatus | null>(null);
  const live = useQrtzEvent('permissions:changed');
  useEffect(() => {
    const read = () =>
      void qrtz.invoke('permissions:get').then(setPerms, () => {});
    read();
    // Granted in System Settings while qrtz was in the background.
    window.addEventListener('focus', read);
    return () => window.removeEventListener('focus', read);
  }, [qrtz]);
  useEffect(() => {
    if (live) setPerms(live);
  }, [live]);
  const ask = (p: OsPermission) =>
    qrtz.invoke('permissions:request', { permission: p }).then(setPerms);
  return { perms, ask };
}

/**
 * ADR 0163 (no onboarding): a module's permissions with their state and "allow", where the user switches it on.
 * Nothing is asked until the user presses allow or first uses the feature.
 */
export function ModulePermissions({
  module,
  perms,
  onAsk,
}: {
  module: ModuleId;
  perms: PermissionsStatus | null;
  onAsk: (p: OsPermission) => void;
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
          };
        const st = perms?.[p as OsPermission] ?? 'unknown';
        const askable = st === 'denied' || st === 'not-determined';
        return {
          id: `${module}-${p}`,
          label,
          state: STATE[st] ?? st,
          ...(askable ? { onGrant: () => onAsk(p as OsPermission) } : {}),
        };
      })}
    />
  );
}
