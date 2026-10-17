import { SubAppManifest } from '@qrtz/core';

/** Pure data: imported by both main (capability checks) and the renderer (sidebar). */
export const manifest = SubAppManifest.parse({
  id: 'home',
  name: 'Home',
  icon: '◆',
  order: 0,
  capabilities: ['app.info'],
});
