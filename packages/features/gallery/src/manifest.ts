import { SubAppManifest } from '@qrtz/core';

/** Design-system gallery: every @qrtz/ui component in every state (M6.1). */
export const manifest = SubAppManifest.parse({
  id: 'gallery',
  name: 'gallery',
  icon: 'grid',
  order: 900,
  capabilities: [],
});
