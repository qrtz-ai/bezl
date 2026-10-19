import { createRegistry } from '@qrtz/core';
import settings from '@qrtz/feature-settings';
import note from '@qrtz/feature-note';
import talk from '@qrtz/feature-talk';
import gallery from '@qrtz/feature-gallery';
import ctrl from '@qrtz/feature-ctrl';
import snap from '@qrtz/feature-snap';
import marketplace from '@qrtz/feature-marketplace';
import dash from '@qrtz/feature-dash';

/** Unpackaged (dev/e2e) builds load the renderer with ?dev=1; packaged builds never list dev-only sub apps. */
export const isDevBuild = () =>
  typeof location !== 'undefined' &&
  new URLSearchParams(location.search).get('dev') === '1';

/**
 * ADR 0228: Agent Dash is behind this flag, off by default: only unpackaged (?dev=1) builds list it, like the
 * gallery, until its server ships.
 */
export const DASH_ENABLED = false;

/** Renderer-side registry. Keep in sync with shared/manifests.ts (checked by a test). */
export const registry = createRegistry(
  isDevBuild()
    ? [talk, note, ctrl, snap, dash, marketplace, gallery, settings]
    : [
        talk,
        note,
        ctrl,
        snap,
        ...(DASH_ENABLED ? [dash] : []),
        marketplace,
        settings,
      ],
);
