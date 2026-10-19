import { SubAppManifest } from '@qrtz/core';

/** qrtz snap (ADR 0159): Spectacle's window snapping, its shortcuts and its settings, as a qrtz module. */
export const manifest = SubAppManifest.parse({
  id: 'snap',
  name: 'snap',
  icon: 'grid',
  order: 40,
  capabilities: ['snap.read', 'module.settings'],
});
