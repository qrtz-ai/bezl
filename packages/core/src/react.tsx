import {
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from 'react';
import type { QrtzClient } from './ipc/client';
import type { EventName, EventPayload } from './ipc/events';

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
