import { z } from 'zod';

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
  /** `name@version` of what depends on it. */
  requiredBy: z.array(z.string()),
  /** Why a revocation switched it off, or null. */
  disabled: z.string().nullable(),
  sizeBytes: z.number().int().nonnegative(),
});
export type PackageRow = z.infer<typeof PackageRow>;

export const PackagesState = z.object({
  packages: z.array(PackageRow),
  /** The stored signed index, or null before the first check. */
  index: z
    .object({ version: z.number().int(), issuedMs: z.number().int() })
    .nullable(),
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

export const PackagesPruned = z.object({
  dirs: z.number().int().nonnegative(),
  blobs: z.number().int().nonnegative(),
  bytes: z.number().int().nonnegative(),
});
export type PackagesPruned = z.infer<typeof PackagesPruned>;
