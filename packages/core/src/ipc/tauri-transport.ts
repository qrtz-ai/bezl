import { AppError } from '../errors';
import type { IpcEnvelope, IpcResult } from './contract';
import type { QrtzTransport } from './client';
import { isStreamEvent } from './events';

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
  /** A Tauri `Channel` (`new Channel()` with `onmessage`) to hand to a command (ADR 0095 streams). */
  channel(onmessage: (payload: unknown) => void): unknown;
}

/** Rust command / event names (apps/desktop/src-tauri/src/commands.rs). */
export const TAURI_INVOKE_CMD = 'qrtz_invoke';
export const TAURI_SUBSCRIBE_CMD = 'qrtz_subscribe';
export const TAURI_EVENT = 'qrtz:event';
export const TAURI_STREAM_OPEN_CMD = 'qrtz_stream_open';
export const TAURI_STREAM_JOIN_CMD = 'qrtz_stream_join';
export const TAURI_STREAM_CLOSE_CMD = 'qrtz_stream_close';

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
 *
 * High-rate events (`stream: true` in the event contract, ADR 0095) don't use the broadcast event: the window opens
 * one `Channel` stream per event with its first listener's sub app, later sub apps join it (each passes the same
 * capability check), and the stream closes when the window's last listener for it goes.
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

  // event → its stream in this window: the backend id (null once refused or closed) and the sub apps it allowed.
  type Stream = { id: Promise<number | null>; allowed: Set<string> };
  const streams = new Map<string, Stream>();
  const ok = (r: unknown) => (r as { ok?: boolean } | null)?.ok === true;
  const deliver = (event: string, payload: unknown, allowed: Set<string>) => {
    for (const l of listeners)
      if (l.event === event && allowed.has(l.subApp))
        try {
          l.cb(payload);
        } catch {
          // isolate listener errors
        }
  };
  const openStream = (subApp: string, event: string): Stream => {
    // Messages only flow once the backend accepted the open, which is for `subApp`: it may hear the first one,
    // even if it lands before the open's reply.
    const allowed = new Set<string>([subApp]);
    const s: Stream = { id: Promise.resolve(null), allowed };
    const channel = bridge.channel((p) => deliver(event, p, allowed));
    s.id = bridge
      .invoke(TAURI_STREAM_OPEN_CMD, { subApp, event, channel })
      .then(
        (r) => (ok(r) ? (r as { id: number }).id : null),
        () => null,
      )
      .then((id) => {
        if (id === null) {
          allowed.clear();
          if (streams.get(event) === s) streams.delete(event);
        }
        return id;
      });
    streams.set(event, s);
    return s;
  };
  const join = async (s: Stream, id: number, subApp: string) => {
    if (s.allowed.has(subApp)) return true;
    const r = await bridge
      .invoke(TAURI_STREAM_JOIN_CMD, { id, subApp })
      .catch(() => null);
    if (!ok(r)) return false;
    s.allowed.add(subApp);
    return true;
  };
  /** Resolves whether `subApp` may receive `event` in this window. */
  const joinStream = async (subApp: string, event: string) => {
    const s = streams.get(event);
    if (!s) return (await openStream(subApp, event).id) !== null;
    const id = await s.id;
    if (id !== null) return join(s, id, subApp);
    // Its opener was refused (or it closed meanwhile): open our own, once.
    const mine = streams.get(event) ?? openStream(subApp, event);
    const id2 = await mine.id;
    return id2 !== null && join(mine, id2, subApp);
  };
  const leaveStream = (event: string) => {
    for (const l of listeners) if (l.event === event) return;
    const s = streams.get(event);
    if (!s) return;
    streams.delete(event);
    void s.id.then(
      (id) =>
        id !== null &&
        bridge.invoke(TAURI_STREAM_CLOSE_CMD, { id }).catch(() => {}),
    );
  };

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
      if (isStreamEvent(l.event)) {
        void joinStream(l.subApp, l.event).then((allowed) => {
          if (!allowed) {
            listeners.delete(l);
            leaveStream(l.event);
          }
        });
        return () => {
          if (listeners.delete(l)) leaveStream(l.event);
        };
      }
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
