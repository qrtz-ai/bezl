import { z } from 'zod';
import { ModuleId } from '../modules';

/**
 * ADR 0157: the package manager as the Extensions screen sees it. Extensions (echo, nano, ctrl), the sandboxed
 * services they use (`inference.*`) and the models those read (`model.*`), installed from the signed index into
 * one content-addressed store. Every change is a plan the user sees (what is added and removed, and the download
 * size) and then confirms by its hash: `packages:apply` takes the plan's `planSha256` and `downloadBytes` back,
 * never a list of packages.
 */
export const PackageKind = z.enum(['extension', 'service', 'model']);
export type PackageKind = z.infer<typeof PackageKind>;

/** Lowercase dotted words: `echo`, `inference.voice`, `model.parakeet-tdt-v2-en`. */
export const PackageName = z
  .string()
  .min(1)
  .max(64)
  .regex(/^[a-z][a-z0-9-]*(\.[a-z0-9][a-z0-9-]*)*$/);

export const PackageRow = z.object({
  name: PackageName,
  version: z.string(),
  kind: PackageKind,
  /** The range the user asked for, or null when it is only here because something needs it. */
  requested: z.string().nullable(),
  /** `name@version` of what depends on it directly in qrtz.lock. */
  requiredBy: z.array(z.string()),
  /**
   * Who uses it, by name (`echo`, `nano`): the extensions above it in qrtz.lock and the modules a model the app
   * pinned is for (including downloads adopted from before the package manager). The host's one answer, the same
   * one `packages:prune` keeps by; empty only for a bare request (`requested` says so).
   */
  usedBy: z.array(z.string()),
  /** Why a revocation switched it off, or null. */
  disabled: z.string().nullable(),
  sizeBytes: z.number().int().nonnegative(),
});
export type PackageRow = z.infer<typeof PackageRow>;

/** The macOS permissions a module uses (ADR 0154: they are qrtz's own; an extension shares them). */
export const ModulePermission = z.enum([
  'microphone',
  'accessibility',
  'screen-recording',
  'automation',
  'files-and-folders',
]);
export type ModulePermission = z.infer<typeof ModulePermission>;

/** Something an extension pulls in (a service or a model), at the version the resolver would pick. */
export const CatalogDependency = z.object({
  name: PackageName,
  range: z.string(),
  /** null: nothing in the index matches the range (the install would fail to resolve). */
  version: z.string().nullable(),
  kind: PackageKind,
  sizeBytes: z.number().int().nonnegative(),
  /** Named in the extension's own qrtz.json (false: needed by one of its dependencies). */
  direct: z.boolean(),
});
export type CatalogDependency = z.infer<typeof CatalogDependency>;

/**
 * ADR 0161: one marketplace card, from the stored signed index (no network): the newest unrevoked version, every
 * dependency it pulls in, the size of all of it together, and the permissions it uses.
 */
export const CatalogEntry = z.object({
  name: PackageName,
  kind: z.literal('extension'),
  version: z.string(),
  /** Every unrevoked version, newest first. */
  versions: z.array(z.string()),
  sizeBytes: z.number().int().nonnegative(),
  /** The extension and every dependency (before what is already installed is subtracted; the plan says that). */
  installSizeBytes: z.number().int().nonnegative(),
  dependencies: z.array(CatalogDependency),
  permissions: z.array(ModulePermission),
  minOs: z.string().nullable(),
  teamId: z.string(),
});
export type CatalogEntry = z.infer<typeof CatalogEntry>;

export const PackagesPruned = z.object({
  dirs: z.number().int().nonnegative(),
  blobs: z.number().int().nonnegative(),
  bytes: z.number().int().nonnegative(),
});
export type PackagesPruned = z.infer<typeof PackagesPruned>;

export const PackagesState = z.object({
  packages: z.array(PackageRow),
  /** The stored signed index, or null before the first check. */
  index: z
    .object({ version: z.number().int(), issuedMs: z.number().int() })
    .nullable(),
  /** ADR 0161: the marketplace catalog (extensions in the stored index); empty before the first check. */
  available: z.array(CatalogEntry),
  /** What `packages:prune` would delete right now (nothing in use): "clean up" is offered only when not zero. */
  reclaimable: PackagesPruned,
});
export type PackagesState = z.infer<typeof PackagesState>;

export const PackageOp = z.discriminatedUnion('op', [
  z.object({
    op: z.literal('install'),
    names: z.array(PackageName).min(1).max(16),
  }),
  z.object({
    op: z.literal('remove'),
    names: z.array(PackageName).min(1).max(16),
  }),
  z.object({
    op: z.literal('update'),
    names: z.array(PackageName).max(16).optional(),
    latest: z.boolean().optional(),
  }),
  /** Switch an extension's engine: its model dependency of the same task becomes `model`. */
  z.object({
    op: z.literal('engine'),
    extension: PackageName,
    model: PackageName,
  }),
]);
export type PackageOp = z.infer<typeof PackageOp>;

export const PackageChange = z.object({
  change: z.enum(['add', 'remove']),
  name: PackageName,
  version: z.string(),
});
export type PackageChange = z.infer<typeof PackageChange>;

export const PackagePlan = z.object({
  planSha256: z.string().regex(/^[0-9a-f]{64}$/),
  changes: z.array(PackageChange),
  downloadBytes: z.number().int().nonnegative(),
});
export type PackagePlan = z.infer<typeof PackagePlan>;

export const PackagesApplied = z.object({
  changes: z.array(PackageChange),
  /** Installed while the revocation list was expired (sideloads only). */
  staleRevocation: z.boolean(),
});
export type PackagesApplied = z.infer<typeof PackagesApplied>;

/** ADR 0161: `packages:apply`'s download, at most once per percent. */
export const PackagesProgress = z.object({
  doneBytes: z.number().int().nonnegative(),
  totalBytes: z.number().int().nonnegative(),
});
export type PackagesProgress = z.infer<typeof PackagesProgress>;

/** ADR 0161: the module manager's switch for one extension. */
export const ModuleEnabled = z.object({
  name: ModuleId,
  enabled: z.boolean(),
});
export type ModuleEnabled = z.infer<typeof ModuleEnabled>;
