import { SubAppManifest } from '@qrtz/core';

/**
 * ADR 0161: the app manager on the qrtz page (installed and available apps, each app's detail modal and models).
 * Pure data: main (capability checks) and the renderer.
 */
export const manifest = SubAppManifest.parse({
  id: 'marketplace',
  // The qrtz page's app manager (its body and each app's modal); never a sidebar row of its own.
  name: 'qrtz',
  icon: 'package',
  order: 950,
  capabilities: [
    // ADR 0157: the package manager (list is local; refresh and install go online only when the user asks).
    'packages.read',
    'packages.manage',
    // The module switches it shows (it writes them only through packages:set-enabled).
    'config.read',
    // ADR 0163 (no onboarding): each app's permissions beside its switch, in its detail modal.
    'os.permissions',
    // Each app's models in its detail modal (moved from settings → speech): status, download, pause, resume and
    // delete, always scoped to that one app (`extensions: [<id>]`); the host keeps sets another app still needs.
    'speech.read',
    'speech.manage',
    // A store that can't be reached shows a friendly line; the raw reason goes to qrtz.log (ADR 0083).
    'log.write',
    // The app's version, shown on bundled echo's row ("v0.2.0 · built in").
    'app.info',
  ],
});
