import { useEffect, useMemo, useState } from 'react';
import { defaultConfig, loadConfig, type Config } from '@qrtz/core';
import { useHiddenModules } from '@qrtz/core/react';
import { shellClient } from './shellClient';

/**
 * ADR 0146: the global module switches from settings → modules, live. Everything is on until config loads. A module
 * whose PostHog flag is off reads as switched off here (no sidebar row, no view, no palette commands); config keeps
 * the user's switch.
 */
export function useModuleSwitches(): Pick<Config, 'modules'> {
  const client = useMemo(shellClient, []);
  const [modules, setModules] = useState<Config['modules']>(
    () => defaultConfig().modules,
  );
  const hidden = useHiddenModules();
  useEffect(() => {
    const apply = (c: Record<string, unknown>) =>
      setModules(loadConfig(c).modules);
    client.invoke('config:get').then(apply, () => {});
    return client.on('config:changed', apply);
  }, [client]);
  return useMemo(() => {
    const off = { ...modules };
    for (const id of hidden) off[id] = { ...off[id], enabled: false };
    return { modules: off };
  }, [modules, hidden]);
}
