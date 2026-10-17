import { createRegistry } from '@qrtz/core';
import settings from '@qrtz/feature-settings';
import note from '@qrtz/feature-note';
import gallery from '@qrtz/feature-gallery';

/** Unpackaged (dev/e2e) builds load the renderer with ?dev=1; packaged builds never list dev-only sub apps. */
export const isDevBuild = () =>
  typeof location !== 'undefined' &&
  new URLSearchParams(location.search).get('dev') === '1';

/** Renderer-side registry. Keep in sync with shared/manifests.ts (checked by a test). */
export const registry = createRegistry(
  isDevBuild() ? [note, gallery, settings] : [note, settings],
);
