import { AppError } from '../errors';
import {
  PROTOCOL_VERSION,
  ipcContract,
  type IpcChannel,
  type IpcEnvelope,
  type IpcRequest,
  type IpcResponse,
  type IpcResult,
} from './contract';
import type { EventName, EventPayload } from './events';

/** What the preload exposes on `window.qrtzTransport`. Deliberately dumb: main validates. */
export interface QrtzTransport {
  invoke(envelope: IpcEnvelope): Promise<IpcResult>;
  on(subApp: string, event: string, cb: (payload: unknown) => void): () => void;
}

export interface QrtzClient {
  readonly subApp: string;
  invoke<C extends IpcChannel>(
    channel: C,
    ...args: IpcRequest<C> extends void ? [] : [IpcRequest<C>]
  ): Promise<IpcResponse<C>>;
  on<E extends EventName>(
    event: E,
    cb: (payload: EventPayload<E>) => void,
  ): () => void;
}

let counter = 0;

/** Typed client bound to one sub app identity. Throws AppError on failure. */
export function createClient(
  transport: QrtzTransport,
  subApp: string,
): QrtzClient {
  return {
    subApp,
    async invoke(channel, ...args) {
      const id = `${subApp}-${Date.now().toString(36)}-${(counter++).toString(36)}`;
      const res = await transport.invoke({
        v: PROTOCOL_VERSION,
        id,
        channel,
        version: ipcContract[channel].version,
        subApp,
        payload: args[0],
      });
      if (!res.ok) throw AppError.fromJSON(res.error);
      return res.data as never;
    },
    on(event, cb) {
      return transport.on(subApp, event, cb as (p: unknown) => void);
    },
  };
}

declare global {
  interface Window {
    /** Injected by the preload script; absent outside the qrtz shell. */
    qrtzTransport?: QrtzTransport;
  }
}
