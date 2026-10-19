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
    // ADR 0163 (no onboarding): each module's permissions beside its switch, and switching echo or nano on starts
    // the speech download when speech isn't here yet.
    'os.permissions',
    'speech.read',
    'speech.manage',
    // A store that can't be reached shows a friendly line; the raw reason goes to qrtz.log (ADR 0083).
    'log.write',
  ],
});
