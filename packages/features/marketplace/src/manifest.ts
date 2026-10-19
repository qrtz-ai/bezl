import { SubAppManifest } from '@qrtz/core';

/** ADR 0161: the marketplace and the module manager. Pure data: main (capability checks) and the renderer. */
export const manifest = SubAppManifest.parse({
  id: 'marketplace',
  name: 'marketplace',
  icon: 'package',
  order: 800,
  capabilities: [
    // ADR 0157: the package manager (list is local; refresh and install go online only when the user asks).
    'packages.read',
    'packages.manage',
    // The module switches it shows (it writes them only through packages:set-enabled).
    'config.read',
  ],
});
