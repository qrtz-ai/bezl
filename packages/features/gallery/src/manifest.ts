import { SubAppManifest } from '@qrtz/core';

/** Design-system gallery: every @qrtz/ui component in every state (M6.1). */
export const manifest = SubAppManifest.parse({
  id: 'gallery',
  name: 'Gallery',
  icon: '▦',
  order: 900,
  capabilities: [],
});
