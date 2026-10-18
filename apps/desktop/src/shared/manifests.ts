import { SHELL_MANIFEST, type SubAppManifest } from '@qrtz/core';
import { manifest as settings } from '@qrtz/feature-settings/manifest';
import { manifest as note } from '@qrtz/feature-note/manifest';
import { manifest as talk } from '@qrtz/feature-talk/manifest';
import { manifest as gallery } from '@qrtz/feature-gallery/manifest';
import { manifest as ctrl } from '@qrtz/feature-ctrl/manifest';

/** Every sub-app manifest known to the app. Main uses this for capability checks. */
export const manifests: readonly SubAppManifest[] = [
  SHELL_MANIFEST,
  note,
  talk,
  ctrl,
  gallery,
  settings,
];

export const resolveManifest = (id: string) =>
  manifests.find((m) => m.id === id);
