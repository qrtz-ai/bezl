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
  // The shell only shows this window while the key is held, or briefly after a failure (crates/qrtz-app
  // src/pill.rs): an idle view whose last dictation couldn't be inserted shows that error. The pill never shows the
  // transcript: a waveform from `level` while listening, a slow swell while processing, or the error.
  const lastError =
    s?.phase === 'idle' && s.last && !s.last.inserted
      ? s.last.error
      : undefined;
  const error = s?.error ?? lastError;
  return (
    <TalkPill
      phase={lastError ? 'error' : (s?.phase ?? 'idle')}
      level={s?.level ?? 0}
      {...(error ? { error } : {})}
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
