import { useEffect, useMemo, useState } from 'react';
import { defaultConfig, loadConfig, type Config } from '@qrtz/core';
import { shellClient } from './shellClient';

/** ADR 0146: the global module switches from settings → modules, live. Everything is on until config loads. */
export function useModuleSwitches(): Pick<Config, 'modules'> {
  const client = useMemo(shellClient, []);
  const [modules, setModules] = useState<Config['modules']>(
    () => defaultConfig().modules,
  );
  useEffect(() => {
    const apply = (c: Record<string, unknown>) =>
      setModules(loadConfig(c).modules);
    client.invoke('config:get').then(apply, () => {});
    return client.on('config:changed', apply);
  }, [client]);
  return useMemo(() => ({ modules }), [modules]);
}
