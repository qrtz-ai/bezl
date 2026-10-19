import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
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

/**
 * A sub app's slice of the shell's route (ADR 0091): `path` is what follows the app id in the hash (e.g.
 * `meetings/m1/summary`). `active` is false while the sub app is mounted but hidden behind another one: gate
 * global listeners on it. While hidden, `navigate` only updates the sub app's remembered path (shown on return)
 * and never switches apps.
 */
export interface SubAppRoute {
  active: boolean;
  path: string;
  navigate: (path: string, opts?: { replace?: boolean }) => void;
}

const RouteContext = createContext<SubAppRoute | null>(null);

export function SubAppRouteProvider({
  route,
  children,
}: {
  route: SubAppRoute;
  children: ReactNode;
}) {
  return (
    <RouteContext.Provider value={route}>{children}</RouteContext.Provider>
  );
}

/** The current sub app's route. Outside the shell (previews, tests) it is local state, always active. */
export function useSubAppRoute(): SubAppRoute {
  const shell = useContext(RouteContext);
  const [path, setPath] = useState('');
  const navigate = useCallback((p: string) => setPath(p), []);
  const local = useMemo(
    () => ({ active: true, path, navigate }),
    [path, navigate],
  );
  return shell ?? local;
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
 * Speech module setup (the settings "speech" section and the modules switches): live status from `speech:status`
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
    client.invoke('speech:status', {}).then((s) => live && setSpeech(s), fail);
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

type SettingsSections = Record<string, Record<string, unknown>>;

/**
 * ADR 0146 M1.4: the calling module's own settings, for the settings page inside its view. Reads with
 * `module:get-settings` and writes one section at a time with `module:set-settings` (optimistic, so a switch
 * follows the click). Needs `module.settings`; the backend scopes it to the view's module.
 */
export function useModuleSettings() {
  const client = useQrtz();
  const [settings, setSettings] = useState<SettingsSections | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    client.invoke('module:get-settings').then(
      (s) => live && setSettings(s as SettingsSections),
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [client]);
  const patch = useCallback(
    (section: string, p: Record<string, unknown>) => {
      setError(null);
      setSettings((prev) =>
        prev ? { ...prev, [section]: { ...prev[section], ...p } } : prev,
      );
      return client
        .invoke('module:set-settings', { patch: { [section]: p } })
        .then(
          (s) => setSettings(s as SettingsSections),
          (e: Error) => setError(e.message),
        );
    },
    [client],
  );
  return { settings, patch, error };
}
