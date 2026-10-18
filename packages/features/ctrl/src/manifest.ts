import { SubAppManifest } from '@qrtz/core';

/** ctrl (ADR 0100, 0103): typed tasks that run in the background, once or on a cadence. No voice, no cloud. */
export const manifest = SubAppManifest.parse({
  id: 'ctrl',
  name: 'ctrl',
  icon: 'command',
  order: 30,
  capabilities: ['ctrl.read', 'ctrl.write', 'module.settings'],
});
