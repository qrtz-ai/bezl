import { useEffect, useMemo, useState } from 'react';
import { appUpdateDot, type AppUpdateStatus } from '@qrtz/core';
import { shellClient } from './shellClient';

/**
 * ADR 0180: the sidebar's "update ready" dot: a newer qrtz is available or downloaded. Reads the host's state
 * (its quiet launch and 6-hour checks); asks no host itself.
 */
export function useAppUpdateDot(): boolean {
  const client = useMemo(shellClient, []);
  const [s, setS] = useState<AppUpdateStatus | null>(null);
  useEffect(() => {
    client.invoke('app-update:status').then(setS, () => {});
    return client.on('app-update:state', setS);
  }, [client]);
  return appUpdateDot(s);
}
