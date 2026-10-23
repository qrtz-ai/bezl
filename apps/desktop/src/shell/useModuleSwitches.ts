import { useEffect, useMemo, useState } from 'react';
import {
  defaultConfig,
  isHiddenExperiment,
  loadConfig,
  MODULE_IDS,
  type Config,
} from '@qrtz/core';
import { shellClient } from './shellClient';

/**
 * An experimental app reads as switched off here (no sidebar row, no view, no palette commands) until settings →
 * app → experimental apps is on; config keeps the user's own switch.
 */
function switchesOf(cfg: Config): Config['modules'] {
  const modules = { ...cfg.modules };
  for (const id of MODULE_IDS)
    if (isHiddenExperiment(cfg, id))
      modules[id] = { ...modules[id], enabled: false };
  return modules;
}

/** ADR 0146: the global module switches from settings → modules, live. Everything is on until config loads. */
export function useModuleSwitches(): Pick<Config, 'modules'> {
  const client = useMemo(shellClient, []);
  // Not switchesOf: hiding an app before config loads would redirect a deep link to it.
  const [modules, setModules] = useState<Config['modules']>(
    () => defaultConfig().modules,
  );
  useEffect(() => {
    const apply = (c: Record<string, unknown>) =>
      setModules(switchesOf(loadConfig(c)));
    client.invoke('config:get').then(apply, () => {});
    return client.on('config:changed', apply);
  }, [client]);
  return useMemo(() => ({ modules }), [modules]);
}
