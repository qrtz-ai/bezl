// Preload: isolated context. Exposes one minimal transport; main validates everything.
import { contextBridge, ipcRenderer } from 'electron';

type Listener = {
  subApp: string;
  event: string;
  cb: (payload: unknown) => void;
};
const listeners = new Set<Listener>();

ipcRenderer.on(
  'qrtz:event',
  (_e, msg: { event: string; payload: unknown; subApps: string[] }) => {
    for (const l of listeners) {
      if (l.event === msg.event && msg.subApps.includes(l.subApp)) {
        try {
          l.cb(msg.payload);
        } catch {
          // isolate listener errors
        }
      }
    }
  },
);

contextBridge.exposeInMainWorld('qrtzTransport', {
  invoke: (envelope: unknown) => ipcRenderer.invoke('qrtz:invoke', envelope),
  on: (subApp: string, event: string, cb: (payload: unknown) => void) => {
    const l: Listener = { subApp: String(subApp), event: String(event), cb };
    listeners.add(l);
    void ipcRenderer
      .invoke('qrtz:subscribe', { subApp: l.subApp, event: l.event })
      .then((r: { ok: boolean }) => {
        if (!r.ok) listeners.delete(l);
      });
    return () => listeners.delete(l);
  },
});
