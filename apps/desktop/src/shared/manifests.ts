import { SHELL_MANIFEST, type SubAppManifest } from '@qrtz/core';
import { manifest as home } from '@qrtz/feature-home/manifest';
import { manifest as settings } from '@qrtz/feature-settings/manifest';

/** Every sub-app manifest known to the app. Main uses this for capability checks. */
export const manifests: readonly SubAppManifest[] = [
  SHELL_MANIFEST,
  home,
  settings,
];

export const resolveManifest = (id: string) =>
  manifests.find((m) => m.id === id);
