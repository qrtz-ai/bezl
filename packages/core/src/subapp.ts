import type { ComponentType } from 'react';
import { z } from 'zod';
import { Capability, OsPermission } from './capabilities';

export const SubAppManifest = z.object({
  /** Stable, unique, kebab-case id; also the route segment and IPC identity. */
  id: z.string().regex(/^[a-z][a-z0-9-]*$/, 'kebab-case id'),
  name: z.string().min(1),
  /** Short glyph or emoji shown in the sidebar. */
  icon: z.string().min(1),
  /** Lower numbers sort first. */
  order: z.number().optional(),
  /** IPC capabilities this sub app may use. Least privilege: declare only what you call. */
  capabilities: z.array(Capability).default([]),
  /** macOS permissions this sub app needs (the module manager shows their state beside its switch; ADR 0163). */
  permissions: z.array(OsPermission).default([]),
});
export type SubAppManifest = z.infer<typeof SubAppManifest>;
export type SubAppManifestInput = z.input<typeof SubAppManifest>;

export interface SubAppLifecycle {
  /** Called when the sub app becomes visible. Return a cleanup to run on deactivate. */
  activate?: () => void | (() => void);
}

export interface SubApp extends SubAppLifecycle {
  manifest: SubAppManifest;
  /** Lazily loaded React entry, so sub apps are code-split. */
  load: () => Promise<{ default: ComponentType }>;
}

export interface SubAppInput extends SubAppLifecycle {
  manifest: SubAppManifestInput;
  load: SubApp['load'];
}

/** Validates the manifest and returns a typed sub app. Throws on invalid manifests. */
export function defineSubApp(input: SubAppInput): SubApp {
  const parsed = SubAppManifest.safeParse(input.manifest);
  if (!parsed.success) {
    throw new Error(
      `Invalid sub app manifest "${String(input.manifest.id)}": ${parsed.error.message}`,
    );
  }
  return { ...input, manifest: parsed.data };
}

/** Built-in identity for the shell itself. */
export const SHELL_MANIFEST: SubAppManifest = {
  id: 'shell',
  name: 'qrtz',
  icon: 'q',
  // The shell hosts the ⌘K palette and the floating recording HUD.
  capabilities: [
    'app.info',
    'config.read',
    'helper.status',
    'note.read',
    'note.record',
    'talk.read',
    'talk.dictate',
    // ADR 0083: uncaught errors and player errors from every window go to the local log.
    'log.write',
    // ADR 0181: the sidebar dot when a newer qrtz is waiting (reads only).
    'app.update.read',
  ],
  permissions: [],
};
