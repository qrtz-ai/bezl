import { z } from 'zod';
import type { Config } from './config';
import type { ModulePermission } from './packages/contract';

/**
 * qrtz modules (ADR 0146). The Rust crate `qrtz-modules` is the authority on manifests and the lockfile; this is the
 * renderer's typed view of the same shapes, plus the mapping from modules to the views they turn on.
 */
export const MODULE_IDS = ['echo', 'nano', 'ctrl', 'snap'] as const;
export const ModuleId = z.enum(MODULE_IDS);
export type ModuleId = z.infer<typeof ModuleId>;

/** The view (sub app id) each module turns on. The views stay in the signed shell (ADR 0078 CSP). */
export const MODULE_VIEWS: Readonly<Record<ModuleId, readonly string[]>> = {
  echo: ['talk'],
  nano: ['note'],
  ctrl: ['ctrl'],
  snap: ['snap'],
};

/**
 * The macOS permissions each module uses (its manifest's `permissions`). ADR 0163: the module manager shows them,
 * with their live state, beside the module's switch; macOS asks for any not granted the first time it is needed.
 */
export const MODULE_PERMISSIONS: Readonly<
  Record<ModuleId, readonly ModulePermission[]>
> = {
  echo: ['microphone', 'accessibility'],
  nano: ['microphone', 'screen-recording', 'accessibility'],
  ctrl: ['accessibility', 'automation', 'files-and-folders'],
  snap: ['accessibility'],
};

/** Config sections that stay global; every other section belongs to one module (`settings.legacySections`). */
export const GLOBAL_CONFIG_SECTIONS = [
  'schemaVersion',
  'theme',
  'logLevel',
  'app',
  'modules',
] as const;

/**
 * Keys inside a module's section that stay global in config.json (ADR 0146 M2.3; crates/qrtz-modules
 * `GLOBAL_KEYS`): the speech downloads serve echo and nano alike, the rest of `speech` is nano's.
 */
export const GLOBAL_CONFIG_KEYS = [
  ['speech', 'autoUpdate'],
  ['speech', 'warmAfterDownload'],
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
        kind: z.enum(['bundled-helper', 'package', 'launch-services']),
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
  return (
    cfg.modules.enabled &&
    cfg.modules[id].installed !== false &&
    cfg.modules[id].enabled
  );
}

/** A bundled module fell back to the copy inside qrtz.app (host-written `modules.<id>.bundledFallback`). */
export function isOnBundledFallback(
  cfg: Pick<Config, 'modules'>,
  id: ModuleId,
): boolean {
  return cfg.modules[id].bundledFallback === true;
}

/** ADR 0169: installed from the marketplace (the host writes it from qrtz.lock). */
export function isModuleInstalled(
  cfg: Pick<Config, 'modules'>,
  id: ModuleId,
): boolean {
  return cfg.modules[id].installed !== false;
}

/**
 * ADR 0174: the modules qrtz.app carries itself (v1: echo). Always installed, never in the marketplace; their view
 * stays in the sidebar while they are off and holds their on/off switch. Same list as qrtz-modules
 * `BUNDLED_EXTENSIONS`.
 */
export const BUNDLED_MODULES: readonly ModuleId[] = ['echo'];

export function isBundledModule(id: string): boolean {
  return (BUNDLED_MODULES as readonly string[]).includes(id);
}

/**
 * Whether a view is shown: shell views always; a bundled module's view always (its switch is in it, ADR 0174);
 * other module views only while their module is on.
 */
export function isViewOn(cfg: Pick<Config, 'modules'>, view: string): boolean {
  const m = moduleOfView(view);
  return m === undefined || isBundledModule(m) || isModuleOn(cfg, m);
}

/** A whole config section (`['talk']`) or one key of a section (`['privacy', 'excludedApps']`). */
export type SettingsPath = readonly [string] | readonly [string, string];

/**
 * ADR 0146 M1.4: what each module's own settings page (inside its view) reads and writes, through
 * `module:get-settings` / `module:set-settings`. The caller's module comes from its view, so echo can't touch
 * nano's settings or the global ones. Storage (M2.3) follows the same lines: `talk` is in echo's file; `note`,
 * `speech.speakerMode` and `privacy` (excluded apps, pause hotkey) in nano's. The speech downloads
 * (`speech.autoUpdate`, `speech.warmAfterDownload`) are shared by echo and nano, so they stay in config.json and
 * the global settings (`GLOBAL_CONFIG_KEYS`). crates/qrtz-app/src/modules.rs has the same table (its test reads this one).
 */
export const MODULE_SETTINGS_PATHS: Readonly<
  Record<ModuleId, readonly SettingsPath[]>
> = {
  echo: [['talk']],
  nano: [
    ['note'],
    ['speech', 'speakerMode'],
    ['privacy', 'excludedApps'],
    ['privacy', 'pauseHotkey'],
  ],
  ctrl: [],
  snap: [['snap']],
};

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === 'object' && v !== null && !Array.isArray(v);

/** The part of the config a module's settings page shows. */
export function pickModuleSettings(
  cfg: Record<string, unknown>,
  id: ModuleId,
): Record<string, unknown> {
  const out: Record<string, Record<string, unknown>> = {};
  for (const [section, key] of MODULE_SETTINGS_PATHS[id]) {
    const v = cfg[section];
    if (!isRecord(v)) continue;
    if (key === undefined) out[section] = { ...v };
    else if (key in v) out[section] = { ...out[section], [key]: v[key] };
  }
  return out;
}

/** Why a patch reaches outside the module's settings, or null when it stays inside them. */
export function modulePatchError(
  id: ModuleId,
  patch: Record<string, unknown>,
): string | null {
  const paths = MODULE_SETTINGS_PATHS[id];
  for (const [section, v] of Object.entries(patch)) {
    if (paths.some(([s, k]) => s === section && k === undefined)) continue;
    const keys = paths
      .filter(([s, k]) => s === section && k !== undefined)
      .map(([, k]) => k);
    if (!keys.length) return `${id} settings can't change ${section}`;
    if (!isRecord(v)) return `${section} must be an object`;
    const bad = Object.keys(v).find((k) => !keys.includes(k));
    if (bad !== undefined)
      return `${id} settings can't change ${section}.${bad}`;
  }
  return null;
}
