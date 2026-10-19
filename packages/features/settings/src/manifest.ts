import { SubAppManifest } from '@qrtz/core';

/** Pure data: imported by both main (capability checks) and the renderer (sidebar). */
export const manifest = SubAppManifest.parse({
  id: 'settings',
  // qrtz's own settings, behind the qrtz page's gear (/qrtz/settings); never a sidebar row of its own.
  name: 'qrtz settings',
  icon: 'settings',
  order: 1000,
  capabilities: [
    'app.info',
    'config.read',
    'config.write',
    'db.read',
    'helper.status',
    'data.backup',
    'speech.read',
    'speech.manage',
    'logs.export',
    'app.update',
    'app.update.read',
  ],
});
