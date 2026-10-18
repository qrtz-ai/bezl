import { SubAppManifest } from '@qrtz/core';

/** Pure data: imported by both main (capability checks) and the renderer (sidebar). */
export const manifest = SubAppManifest.parse({
  id: 'settings',
  name: 'settings',
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
    // ADR 0157: the Extensions screen.
    'packages.read',
    'packages.manage',
  ],
});
