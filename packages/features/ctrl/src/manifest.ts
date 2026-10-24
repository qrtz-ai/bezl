import { SubAppManifest } from '@qrtz/core';

/** ctrl do (ADR 0214): type one command, ctrl does it on screen over the accessibility tree. No voice, no cloud. */
export const manifest = SubAppManifest.parse({
  id: 'ctrl',
  name: 'ctrl',
  icon: 'command',
  order: 30,
  capabilities: [
    'ctrl.read',
    'ctrl.write',
    'config.read',
    'config.write',
    'module.settings',
  ],
});
