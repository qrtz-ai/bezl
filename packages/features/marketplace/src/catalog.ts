import {
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
    icon: 'edit',
  },
  ctrl: {
    description:
      'tasks on a schedule. tidies folders, closes tabs and quits apps, and asks first.',
    icon: 'command',
  },
  snap: {
    description:
      'window snapping. move and size windows to halves, thirds and corners from the keyboard.',
    icon: 'grid',
  },
};

export const isModuleId = (n: string): n is ModuleId =>
  (MODULE_IDS as readonly string[]).includes(n);

export function infoFor(name: string): ExtensionInfo {
  if (isModuleId(name))
    return { ...KNOWN[name], view: MODULE_VIEWS[name][0] ?? null };
  return { description: 'a qrtz extension.', icon: 'package', view: null };
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
 * installed (in qrtz.lock, newest) · update (the index has a newer one) · built-in (ships inside this qrtz build
 * and isn't installed as a package) · available.
 */
export function statusOf(
  name: string,
  entry: CatalogEntry | undefined,
  rows: readonly PackageRow[],
): ExtensionStatus {
  const row = installedRow(name, rows);
  if (row)
    return entry && compareVersions(entry.version, row.version) > 0
      ? 'update'
      : 'installed';
  if (isModuleId(name)) return 'built-in';
  return 'available';
}

/** `628 mb with dependencies` / `9 mb` / `included in qrtz`. */
export function sizeLine(entry: CatalogEntry | undefined): string {
  if (!entry) return 'included in qrtz';
  return entry.dependencies.length
    ? `${formatBytes(entry.installSizeBytes)} with dependencies`
    : formatBytes(entry.sizeBytes);
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
 * Every extension the marketplace shows: the index's, then the ones built into this build that the index doesn't
 * list yet (offline, or before the first check), so the marketplace is never empty for no reason.
 */
export function listings(
  available: readonly CatalogEntry[],
): { name: string; entry: CatalogEntry | undefined }[] {
  const out: { name: string; entry: CatalogEntry | undefined }[] =
    available.map((e) => ({ name: e.name, entry: e }));
  for (const id of MODULE_IDS)
    if (!available.some((e) => e.name === id))
      out.push({ name: id, entry: undefined });
  return out;
}

/** The marketplace's sub-routes: browse (''), `installed` (the module manager), `item/<name>` (a detail page). */
export type MarketRoute =
  | { page: 'browse' }
  | { page: 'installed' }
  | { page: 'item'; name: string };

export function parseMarketRoute(path: string): MarketRoute {
  const [a, b] = path.split('/').filter(Boolean);
  if (a === 'installed') return { page: 'installed' };
  if (a === 'item' && b && /^[a-z][a-z0-9.-]{0,63}$/.test(b))
    return { page: 'item', name: b };
  return { page: 'browse' };
}

export const marketPath = (r: MarketRoute) =>
  r.page === 'browse'
    ? ''
    : r.page === 'installed'
      ? 'installed'
      : `item/${r.name}`;
