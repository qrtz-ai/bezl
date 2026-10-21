import {
  isBundledModule,
  MODULE_IDS,
  MODULE_VIEWS,
  type CatalogEntry,
  type ModuleId,
  type ModulePermission,
  type PackageKind,
  type PackageRow,
} from '@qrtz/core';
import { formatBytes, type ExtensionStatus, type IconName } from '@qrtz/ui';

/**
 * ADR 0161: what the marketplace and the module manager say about an extension. The index is signed data about
 * packages (versions, sizes, dependencies); the words and glyphs for the extensions this build knows live here.
 * An extension this build doesn't know yet still lists, with a plain line and the package glyph.
 */
export interface ExtensionInfo {
  description: string;
  icon: IconName;
  /** The sub app it turns on (its own settings live there), or null when this build has no view for it. */
  view: string | null;
}

const KNOWN: Record<ModuleId, Omit<ExtensionInfo, 'view'>> = {
  echo: {
    description:
      'dictation anywhere. hold fn, talk, and it types for you, on this mac.',
    icon: 'mic',
  },
  nano: {
    description:
      'meeting notes. records, transcribes and summarizes calls without leaving this mac.',
    icon: 'edit-3',
  },
  ctrl: {
    description:
      'tasks on a schedule. tidies folders, closes tabs and quits apps, and asks first.',
    icon: 'command',
  },
  snap: {
    description:
      'window snapping. move and size windows to halves, thirds and corners from the keyboard.',
    icon: 'layout',
  },
};

export const isModuleId = (n: string): n is ModuleId =>
  (MODULE_IDS as readonly string[]).includes(n);

export function infoFor(name: string): ExtensionInfo {
  if (isModuleId(name))
    return { ...KNOWN[name], view: MODULE_VIEWS[name][0] ?? null };
  return { description: 'a qrtz app.', icon: 'package', view: null };
}

export const PERMISSIONS: Record<
  ModulePermission,
  { label: string; icon: IconName }
> = {
  microphone: { label: 'microphone', icon: 'mic' },
  accessibility: { label: 'accessibility', icon: 'accessibility' },
  'screen-recording': {
    label: 'screen & system audio recording',
    icon: 'screen-recording',
  },
  automation: { label: 'automation', icon: 'automation' },
  'files-and-folders': { label: 'files and folders', icon: 'folder' },
};

export const KIND_ICON: Record<PackageKind, IconName> = {
  extension: 'package',
  service: 'service',
  model: 'model',
};

/** -1 / 0 / 1 on MAJOR.MINOR.PATCH (a pre-release sorts before its release). */
export function compareVersions(a: string, b: string): number {
  const parse = (v: string) => {
    const [core = '', pre] = v.split('-', 2);
    return { n: core.split('.').map((x) => Number(x) || 0), pre };
  };
  const x = parse(a);
  const y = parse(b);
  for (let i = 0; i < 3; i++) {
    const d = (x.n[i] ?? 0) - (y.n[i] ?? 0);
    if (d) return Math.sign(d);
  }
  if (x.pre === y.pre) return 0;
  if (!x.pre) return 1;
  if (!y.pre) return -1;
  return x.pre < y.pre ? -1 : 1;
}

/** The installed extension row for `name`, if the lock has it. */
export const installedRow = (name: string, rows: readonly PackageRow[]) =>
  rows.find((r) => r.kind === 'extension' && r.name === name);

/**
 * installed (in qrtz.lock, newest) · update (the index has a newer one) · available. ADR 0169: qrtz ships empty, so
 * nothing is "built in". `installed` is the host's `modules.<id>.installed` for a module with no lock row (only the
 * mock backend or a dev monolith, where every module counts as installed).
 */
export function statusOf(
  name: string,
  entry: CatalogEntry | undefined,
  rows: readonly PackageRow[],
  installed = false,
): ExtensionStatus {
  const row = installedRow(name, rows);
  if (row)
    return entry && compareVersions(entry.version, row.version) > 0
      ? 'update'
      : 'installed';
  if (installed && isModuleId(name)) return 'installed';
  return 'available';
}

/** `628 mb with dependencies` / `9 mb` / `on this mac` (installed, not in the index). */
export function sizeLine(entry: CatalogEntry | undefined): string {
  if (!entry) return 'on this mac';
  return entry.dependencies.length
    ? `${formatBytes(entry.installSizeBytes)} with dependencies`
    : formatBytes(entry.sizeBytes);
}

/**
 * A card's version line. From the index: `v0.3.0 · 628 mb with dependencies`. A bundled module (echo, ADR 0174)
 * with no index entry ships in qrtz.app, so it carries the app's version: `v0.2.0 · built in` (never
 * `vinstalled`). Anything else installed without an entry: `installed · on this mac`.
 */
export function metaLine(
  name: string,
  entry: CatalogEntry | undefined,
  appVersion?: string | null,
): string {
  if (entry) return `v${entry.version} · ${sizeLine(entry)}`;
  if (isBundledModule(name))
    return appVersion ? `v${appVersion} · built in` : 'built in';
  return 'installed · on this mac';
}

/** What `name@version` pulls in, from the lock (services and models whose requiredBy names it). */
export function lockedDependencies(
  row: PackageRow,
  rows: readonly PackageRow[],
): PackageRow[] {
  const me = `${row.name}@${row.version}`;
  return rows.filter(
    (r) => r.kind !== 'extension' && r.requiredBy.includes(me),
  );
}

/**
 * Every extension the marketplace shows: the index's, then the installed modules it doesn't list (offline, or
 * before the first check). ADR 0169: nothing else; a module that isn't installed shows only from the index.
 */
export function listings(
  available: readonly CatalogEntry[],
  installed: readonly string[] = [],
): { name: string; entry: CatalogEntry | undefined }[] {
  const out: { name: string; entry: CatalogEntry | undefined }[] =
    available.map((e) => ({ name: e.name, entry: e }));
  for (const id of MODULE_IDS)
    if (installed.includes(id) && !available.some((e) => e.name === id))
      out.push({ name: id, entry: undefined });
  return out;
}

/** The qrtz page's sub-routes: the app list (''), and `item/<name>` (the list with that app's detail modal open). */
export type MarketRoute = { page: 'list' } | { page: 'item'; name: string };

export function parseMarketRoute(path: string): MarketRoute {
  const [a, b] = path.split('/').filter(Boolean);
  if (a === 'item' && b && /^[a-z][a-z0-9.-]{0,63}$/.test(b))
    return { page: 'item', name: b };
  return { page: 'list' };
}

export const marketPath = (r: MarketRoute) =>
  r.page === 'item' ? `item/${r.name}` : '';

/** The apps whose models the scoped speech calls reach (`extensions: [<id>]`, ADR 0175). */
export const MODEL_APPS = ['echo', 'nano', 'ctrl'] as const;
export type ModelApp = (typeof MODEL_APPS)[number];
export const isModelApp = (n: string): n is ModelApp =>
  (MODEL_APPS as readonly string[]).includes(n);

/**
 * Each app's model sets: its `model:` dependencies in modules/<id>/module.json (checked by catalog.test), which
 * the host's `sets_needed_by` reads. A scoped `speech:delete` removes only the sets no other switched-on app
 * needs; `keptFor` says which stay and why.
 */
export const MODEL_SETS: Record<ModelApp, readonly string[]> = {
  echo: ['parakeet-tdt-v2'],
  nano: ['nemotron-560ms', 'nemotron3-diarizer', 'localvqe'],
  ctrl: ['laya'],
};

/** For each of `app`'s sets, the other switched-on apps that also need it (so deleting `app`'s models keeps it). */
export function keptFor(
  app: ModelApp,
  on: readonly string[],
  sets: Record<ModelApp, readonly string[]> = MODEL_SETS,
): { name: string; keptFor: string[] }[] {
  return sets[app].map((name) => ({
    name,
    keptFor: MODEL_APPS.filter(
      (o) => o !== app && on.includes(o) && sets[o].includes(name),
    ),
  }));
}
