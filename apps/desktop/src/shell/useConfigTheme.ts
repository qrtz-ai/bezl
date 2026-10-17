import { useEffect, useMemo, useState } from 'react';
import type { ThemePreference } from '@qrtz/ui';
import { shellClient } from './shellClient';

/** Appearance from Settings, live (dark is the qrtz.ai default; light and system are opt-in). Used by every window. */
export function useConfigTheme(): ThemePreference {
  const client = useMemo(shellClient, []);
  const [theme, setTheme] = useState<ThemePreference>('dark');
  useEffect(() => {
    const apply = (c: Record<string, unknown>) => {
      const v = c.theme;
      if (v === 'dark' || v === 'light' || v === 'system') setTheme(v);
    };
    client.invoke('config:get').then(apply, () => {});
    return client.on('config:changed', apply);
  }, [client]);
  return theme;
}
