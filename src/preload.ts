// Preload runs in an isolated context. Expose only a minimal, explicit API.
import { contextBridge } from 'electron';

contextBridge.exposeInMainWorld('qrtz', {
  platform: process.platform,
});
