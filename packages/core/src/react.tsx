import {
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from 'react';
import type { QrtzClient } from './ipc/client';
import type { EventName, EventPayload } from './ipc/events';
import type { SpeechSetupStatus } from './speech/setup';

const ClientContext = createContext<QrtzClient | null>(null);

export function QrtzClientProvider({
  client,
  children,
}: {
  client: QrtzClient;
  children: ReactNode;
}) {
  return (
    <ClientContext.Provider value={client}>{children}</ClientContext.Provider>
  );
}

/** The IPC client scoped to the current sub app's identity and capabilities. */
export function useQrtz(): QrtzClient {
  const c = useContext(ClientContext);
  if (!c)
    throw new Error(
      'useQrtz() must be used inside a sub app rendered by the qrtz shell',
    );
  return c;
}

/** Subscribe to a main → renderer event; returns the latest payload. */
export function useQrtzEvent<E extends EventName>(
  event: E,
  initial?: EventPayload<E>,
): EventPayload<E> | undefined {
  const client = useQrtz();
  const [v, setV] = useState(initial);
  useEffect(() => client.on(event, setV), [client, event]);
  return v;
}

/**
 * Speech module setup (onboarding and the settings "speech" section): live status from `speech:status`
 * events and the actions. Needs `speech.read` (+ `speech.manage` to act).
 *
 * ADR 0078: opening a screen never goes online. `speech:check` (the release list) runs only from `check()`,
 * which the user starts; `_opts` is accepted for older callers and ignored.
 */
export function useSpeechSetup(_opts: { check?: boolean } = {}) {
  const client = useQrtz();
  const [speech, setSpeech] = useState<SpeechSetupStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    const fail = (e: Error) => live && setError(e.message);
    client.invoke('speech:status').then((s) => live && setSpeech(s), fail);
    const offs = [client.on('speech:status', (s) => setSpeech(s))];
    return () => {
      live = false;
      offs.forEach((off) => off());
    };
  }, [client]);
  const act =
    <T,>(p: () => Promise<T>, set: (v: T) => void) =>
    () => {
      setError(null);
      p().then(set, (e: Error) => setError(e.message));
    };
  return {
    speech,
    error,
    download: (force = false) =>
      act(() => client.invoke('speech:download', { force }), setSpeech)(),
    pause: act(() => client.invoke('speech:pause'), setSpeech),
    resume: act(() => client.invoke('speech:resume'), setSpeech),
    remove: act(() => client.invoke('speech:delete'), setSpeech),
    /** The user asked to check for a newer speech module (the only time a screen looks online). */
    check: act(() => client.invoke('speech:check'), setSpeech),
  };
}
