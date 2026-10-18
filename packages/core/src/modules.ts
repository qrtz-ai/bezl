import { z } from 'zod';
import type { Config } from './config';

/**
 * qrtz modules (ADR 0146). The Rust crate `qrtz-modules` is the authority on manifests and the lockfile; this is the
 * renderer's typed view of the same shapes, plus the mapping from modules to the views they turn on.
 */
export const MODULE_IDS = ['echo', 'nano', 'ctrl'] as const;
export const ModuleId = z.enum(MODULE_IDS);
export type ModuleId = z.infer<typeof ModuleId>;

/** The view (sub app id) each module turns on. The views stay in the signed shell (ADR 0078 CSP). */
export const MODULE_VIEWS: Readonly<Record<ModuleId, readonly string[]>> = {
  echo: ['talk'],
  nano: ['note'],
  ctrl: ['ctrl'],
};

/** Config sections that stay global; every other section belongs to one module (`settings.legacySections`). */
export const GLOBAL_CONFIG_SECTIONS = [
  'schemaVersion',
  'theme',
  'logLevel',
  'app',
  'onboarding',
  'privacy',
  'modules',
] as const;

/** An exact version: MAJOR.MINOR.PATCH with an optional pre-release. No ranges, no build metadata. */
export const ExactVersion = z
  .string()
  .regex(
    /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$/,
    'an exact version (no ranges)',
  );
const Sha256 = z.string().regex(/^[0-9a-f]{64}$/);
const TeamId = z.string().regex(/^[0-9A-Z]{10}$/);

/** A pinned dependency. Shared code is only ever an xpc-service in its own process; data is read-only files. */
export const ModuleDependency = z.discriminatedUnion('kind', [
  z
    .object({
      id: z.string().regex(/^model:[a-z][a-z0-9-]*$/),
      kind: z.literal('data'),
      version: ExactVersion,
      sha256: Sha256,
      teamId: TeamId,
      optional: z.boolean().optional(),
    })
    .strict(),
  z
    .object({
      id: z.string().regex(/^xpc:[a-z][a-z0-9-]*$/),
      kind: z.literal('xpc-service'),
      version: ExactVersion,
      sha256: Sha256,
      teamId: TeamId,
      bundleId: z.string().regex(/^ai\.qrtz\.inference\.[a-z][a-z0-9-]*$/),
      designatedRequirement: z.string().min(1),
      optional: z.boolean().optional(),
    })
    .strict(),
]);
export type ModuleDependency = z.infer<typeof ModuleDependency>;

export const ModuleManifest = z
  .object({
    schema: z.literal(1),
    id: ModuleId,
    name: z.string().min(1),
    version: ExactVersion,
    protocol: z.number().int().min(1),
    minApp: ExactVersion,
    maxApp: ExactVersion.optional(),
    code: z
      .object({
        bundleId: z.string(),
        teamId: TeamId,
        designatedRequirement: z.string().min(1),
      })
      .strict(),
    launch: z
      .object({
        kind: z.enum(['bundled-helper', 'launch-services']),
        entry: z.string().min(1),
        channels: z.array(z.string()).optional(),
      })
      .strict(),
    views: z.array(z.string()),
    capabilities: z.array(z.string()),
    permissions: z.array(
      z.enum([
        'microphone',
        'accessibility',
        'screen-recording',
        'automation',
        'files-and-folders',
      ]),
    ),
    entitlements: z.array(z.string()),
    settings: z
      .object({
        namespace: z.string(),
        legacySections: z.array(z.string()).optional(),
      })
      .strict(),
    provides: z.array(z.string()).optional(),
    dependencies: z.array(ModuleDependency),
  })
  .strict();
export type ModuleManifest = z.infer<typeof ModuleManifest>;

/** The module a view belongs to, or undefined for shell views (settings, gallery). */
export function moduleOfView(view: string): ModuleId | undefined {
  return MODULE_IDS.find((m) => MODULE_VIEWS[m].includes(view));
}

/** Whether a module is switched on in the global settings (the all-modules switch wins). */
export function isModuleOn(
  cfg: Pick<Config, 'modules'>,
  id: ModuleId,
): boolean {
  return cfg.modules.enabled && cfg.modules[id].enabled;
}

/** Whether a view is shown: shell views always; module views only while their module is on. */
export function isViewOn(cfg: Pick<Config, 'modules'>, view: string): boolean {
  const m = moduleOfView(view);
  return m === undefined || isModuleOn(cfg, m);
}
