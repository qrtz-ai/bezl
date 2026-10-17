import type { ComponentType } from 'react';

/** Static description of a sub app. Kept serialisable so the shell can list
 * sub apps (sidebar, command palette) without mounting them. */
export interface SubAppManifest {
  /** Stable, unique, kebab-case id. Also used as the route segment. */
  id: string;
  name: string;
  /** Short glyph or emoji shown in the sidebar. */
  icon: string;
  /** Lower numbers sort first in the sidebar. */
  order?: number;
}

export interface SubApp {
  manifest: SubAppManifest;
  /** Lazily loaded React entry, so sub apps are code-split. */
  load: () => Promise<{ default: ComponentType }>;
}

/** Identity helper that gives sub-app authors type checking. */
export function defineSubApp(subApp: SubApp): SubApp {
  if (!/^[a-z][a-z0-9-]*$/.test(subApp.manifest.id)) {
    throw new Error(`Invalid sub app id "${subApp.manifest.id}"`);
  }
  return subApp;
}
