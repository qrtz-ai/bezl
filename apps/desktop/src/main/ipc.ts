import { app, ipcMain, type WebContents } from 'electron';
import { z } from 'zod';
import {
  AppError,
  createRouter,
  eventContract,
  isEventName,
  type EventBus,
  type EventName,
  type Handlers,
  type Logger,
} from '@qrtz/core';
import { resolveManifest } from '../shared/manifests';

const Subscribe = z.object({
  subApp: z.string().max(64),
  event: z.string().max(64),
});

/**
 * Wires the validated router onto Electron IPC and fans bus events out to
 * subscribed sub apps that hold the event's capability.
 */
export function registerIpc(opts: {
  handlers: Partial<Handlers>;
  bus: EventBus;
  log: Logger;
  isTrustedSender: (url: string) => boolean;
}) {
  const log = opts.log.child('ipc');
  const router = createRouter({
    handlers: opts.handlers,
    resolveSubApp: resolveManifest,
    logger: log,
  });
  const subs = new Map<
    number,
    { wc: WebContents; byEvent: Map<EventName, Set<string>> }
  >();

  const checkSender = (url: string | undefined) => {
    if (!url || !opts.isTrustedSender(url))
      throw new AppError('E_FORBIDDEN', 'untrusted sender');
  };

  ipcMain.handle('qrtz:invoke', async (e, envelope: unknown) => {
    try {
      checkSender(e.senderFrame?.url);
    } catch (err) {
      return { ok: false, id: 'unknown', error: AppError.from(err).toJSON() };
    }
    return router.dispatch(envelope);
  });

  ipcMain.handle('qrtz:subscribe', (e, raw: unknown) => {
    try {
      checkSender(e.senderFrame?.url);
      const { subApp, event } = Subscribe.parse(raw);
      if (!isEventName(event))
        throw new AppError('E_UNKNOWN_CHANNEL', `Unknown event "${event}"`);
      const m = resolveManifest(subApp);
      if (!m || !m.capabilities.includes(eventContract[event].capability)) {
        throw new AppError(
          'E_FORBIDDEN',
          `"${subApp}" may not subscribe to "${event}"`,
        );
      }
      let entry = subs.get(e.sender.id);
      if (!entry) {
        entry = { wc: e.sender, byEvent: new Map() };
        subs.set(e.sender.id, entry);
        e.sender.once('destroyed', () => subs.delete(e.sender.id));
      }
      let set = entry.byEvent.get(event);
      if (!set) entry.byEvent.set(event, (set = new Set()));
      set.add(subApp);
      return { ok: true };
    } catch (err) {
      return { ok: false, error: AppError.from(err).toJSON() };
    }
  });

  for (const event of Object.keys(eventContract) as EventName[]) {
    opts.bus.on(event, (payload) => {
      for (const { wc, byEvent } of subs.values()) {
        const allowed = byEvent.get(event);
        if (allowed?.size && !wc.isDestroyed())
          wc.send('qrtz:event', { event, payload, subApps: [...allowed] });
      }
    });
  }

  app.on('will-quit', () => {
    ipcMain.removeHandler('qrtz:invoke');
    ipcMain.removeHandler('qrtz:subscribe');
  });
}
