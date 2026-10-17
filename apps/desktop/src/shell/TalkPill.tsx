import { useEffect, useMemo, useState } from 'react';
import { TalkPill, ThemeProvider } from '@qrtz/ui';
import type { TalkView } from '@qrtz/core';
import { shellClient } from './shellClient';
import { useConfigTheme } from './useConfigTheme';

function Pill() {
  const client = useMemo(shellClient, []);
  const [s, setS] = useState<TalkView | null>(null);
  useEffect(() => {
    client.invoke('talk:state').then(setS, () => {});
    return client.on('talk:state', setS);
  }, [client]);
  return (
    <TalkPill
      phase={s?.phase ?? 'idle'}
      level={s?.level ?? 0}
      text={s?.text ?? ''}
      partial={s?.partial ?? ''}
      {...(s?.error ? { error: s.error } : {})}
    />
  );
}

/** Root for the floating dictation pill window (index.html#pill). */
export function TalkPillApp() {
  const theme = useConfigTheme();
  return (
    <ThemeProvider mode={theme}>
      <Pill />
    </ThemeProvider>
  );
}
