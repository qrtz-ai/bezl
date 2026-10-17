import { AppError } from '../errors';
import type { IpcEnvelope, IpcResult } from './contract';
import type { QrtzTransport } from './client';

/**
 * The two Tauri primitives the transport needs. The desktop-tauri app passes `invoke` from
 * `@tauri-apps/api/core` and `listen` from `@tauri-apps/api/event`; tests pass fakes. Core stays
 * free of any Tauri dependency.
 */
export interface TauriBridge {
  invoke(cmd: string, args?: Record<string, unknown>): Promise<unknown>;
  listen(
    event: string,
    cb: (e: { payload: unknown }) => void,
  ): Promise<() => void>;
}

/** Rust command / event names (apps/desktop-tauri/src-tauri/src/commands.rs). */
export const TAURI_INVOKE_CMD = 'qrtz_invoke';
export const TAURI_SUBSCRIBE_CMD = 'qrtz_subscribe';
export const TAURI_EVENT = 'qrtz:event';

interface EventMsg {
  event: string;
  payload: unknown;
  subApps: string[];
}
const isEventMsg = (m: unknown): m is EventMsg =>
  typeof m === 'object' &&
  m !== null &&
  typeof (m as EventMsg).event === 'string' &&
  Array.isArray((m as EventMsg).subApps);

/**
 * `QrtzTransport` over Tauri IPC. Same semantics as the Electron preload (apps/desktop/src/preload.ts):
 * the backend validates envelopes, checks capabilities and fans events out only to sub apps that
 * subscribed and hold the event's capability. A rejected Tauri invoke (backend gone, command
 * missing, webview not allowed) comes back as a retryable E_UNAVAILABLE result, never a throw.
 */
export function createTauriTransport(bridge: TauriBridge): QrtzTransport {
  type Listener = {
    subApp: string;
    event: string;
    cb: (p: unknown) => void;
  };
  const listeners = new Set<Listener>();
  let listening: Promise<unknown> | null = null;
  const ensureListening = () =>
    (listening ??= bridge
      .listen(TAURI_EVENT, ({ payload }) => {
        if (!isEventMsg(payload)) return;
        for (const l of listeners) {
          if (l.event === payload.event && payload.subApps.includes(l.subApp)) {
            try {
              l.cb(payload.payload);
            } catch {
              // isolate listener errors
            }
          }
        }
      })
      .catch(() => {
        listening = null; // retry on the next subscription
      }));

  return {
    async invoke(envelope: IpcEnvelope): Promise<IpcResult> {
      try {
        return (await bridge.invoke(TAURI_INVOKE_CMD, {
          envelope,
        })) as IpcResult;
      } catch (err) {
        const msg =
          err instanceof Error ? err.message : String(err ?? 'unknown');
        return {
          ok: false,
          id: envelope.id,
          error: new AppError('E_UNAVAILABLE', `tauri invoke failed: ${msg}`, {
            retryable: true,
          }).toJSON(),
        };
      }
    },
    on(subApp, event, cb) {
      const l: Listener = { subApp: String(subApp), event: String(event), cb };
      listeners.add(l);
      void ensureListening();
      bridge
        .invoke(TAURI_SUBSCRIBE_CMD, { subApp: l.subApp, event: l.event })
        .then((r) => {
          if (!(r as { ok?: boolean } | null)?.ok) listeners.delete(l);
        })
        .catch(() => listeners.delete(l));
      return () => listeners.delete(l);
    },
  };
}
