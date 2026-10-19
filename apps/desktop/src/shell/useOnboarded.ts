import { useEffect, useMemo, useState } from 'react';
import { shellClient } from './shellClient';

/**
 * ADR 0184: `onboarding.done` from config (host-written). `false` shows the permissions onboarding before the qrtz
 * page; `true` the app. Until config answers (or when it can't, e.g. outside qrtz) the app shows, so a backend
 * hiccup never traps the user in onboarding.
 */
export function useOnboarded(): [boolean | null, (v: boolean) => void] {
  const client = useMemo(shellClient, []);
  const [done, setDone] = useState<boolean | null>(null);
  useEffect(() => {
    client.invoke('config:get').then(
      (c) =>
        setDone(
          (c.onboarding as { done?: boolean } | undefined)?.done ?? false,
        ),
      () => setDone(true),
    );
  }, [client]);
  return [done, setDone];
}
