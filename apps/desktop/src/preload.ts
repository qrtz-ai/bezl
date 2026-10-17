// Preload runs in an isolated context. Expose only the typed bridge.
import { contextBridge, ipcRenderer } from 'electron';
import { isIpcChannel, type QrtzBridge } from '@qrtz/core';

const bridge: QrtzBridge = {
  invoke: (channel, ...args) => {
    if (!isIpcChannel(channel)) throw new Error(`Unknown channel ${channel}`);
    return ipcRenderer.invoke(channel, ...args);
  },
};

contextBridge.exposeInMainWorld('qrtz', bridge);
