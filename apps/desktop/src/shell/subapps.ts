import { createRegistry } from '@qrtz/core';
import home from '@qrtz/feature-home';
import settings from '@qrtz/feature-settings';
import memo from '@qrtz/feature-memo';
import gallery from '@qrtz/feature-gallery';

/** Renderer-side registry. Keep in sync with shared/manifests.ts (checked by a test). */
export const registry = createRegistry([home, memo, gallery, settings]);
