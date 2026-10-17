import { createRegistry } from '@qrtz/core';
import home from '@qrtz/feature-home';
import settings from '@qrtz/feature-settings';

/** Renderer-side registry. Keep in sync with shared/manifests.ts (checked by a test). */
export const registry = createRegistry([home, settings]);
