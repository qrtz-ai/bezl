// The mocked backend's package manager (e2e and `dev:mock`, ADR 0157/0161): the golden catalog, an in-memory
// lock, plans that add what an install needs and drop what nothing needs any more, `packages:apply` only for the
// plan that was shown, and a download that reports `packages:progress`. Nothing is fetched. The real resolver,
// store and verify path are covered by `crates/qrtz-app/tests/module_hosting.rs` and `tests/marketplace.rs`.
import {
  AppError,
  type CatalogEntry,
  type Handlers,
  type ModuleId,
  type PackageChange,
  type PackageRow,
} from '@qrtz/core';
import { ipcSamples } from '@qrtz/core/ipc/samples';

type Emit = (event: 'packages:progress', payload: unknown) => void;

const sha = (s: string) => {
  let h = 0x811c9dc5;
  for (const c of s) h = Math.imul(h ^ c.charCodeAt(0), 0x01000193) >>> 0;
  return h.toString(16).padStart(8, '0').repeat(8);
};

export function packagesHandlers(
  emit: Emit,
  setEnabled: (name: ModuleId, on: boolean) => boolean,
  tickMs = 40,
): Partial<Handlers> {
  const base = ipcSamples['packages:list'].response;
  const catalog: CatalogEntry[] = base.available;
  const index = base.index;
  /** Requested extensions → their version. */
  let requested = new Map<string, string>(
    base.packages
      .filter((p) => p.requested)
      .map((p) => [p.name, p.version] as const),
  );
  let plan: {
    planSha256: string;
    changes: PackageChange[];
    downloadBytes: number;
    next: Map<string, string>;
  } | null = null;

  const rowsFor = (req: Map<string, string>): PackageRow[] => {
    const rows = new Map<string, PackageRow>();
    for (const [name, version] of req) {
      const c = catalog.find((e) => e.name === name);
      if (!c) continue;
      const me = `${name}@${version}`;
      rows.set(name, {
        name,
        version,
        kind: 'extension',
        requested: `^${version}`,
        requiredBy: [],
        disabled: null,
        sizeBytes: c.sizeBytes,
      });
      for (const d of c.dependencies) {
        if (!d.version) continue;
        const r = rows.get(d.name) ?? {
          name: d.name,
          version: d.version,
          kind: d.kind,
          requested: null,
          requiredBy: [],
          disabled: null,
          sizeBytes: d.sizeBytes,
        };
        r.requiredBy = [...r.requiredBy, me];
        rows.set(d.name, r);
      }
    }
    return [...rows.values()];
  };
  const state = () => ({
    packages: rowsFor(requested),
    index,
    available: catalog,
  });
  const diff = (next: Map<string, string>) => {
    const before = new Map(rowsFor(requested).map((r) => [r.name, r]));
    const after = new Map(rowsFor(next).map((r) => [r.name, r]));
    const changes: PackageChange[] = [];
    let downloadBytes = 0;
    for (const [n, r] of after)
      if (before.get(n)?.version !== r.version) {
        changes.push({ change: 'add', name: n, version: r.version });
        downloadBytes += r.sizeBytes;
      }
    for (const [n, r] of before)
      if (after.get(n)?.version !== r.version)
        changes.push({ change: 'remove', name: n, version: r.version });
    return { changes, downloadBytes };
  };

  return {
    'packages:list': () => state(),
    'packages:refresh': () => state(),
    'packages:plan': (op) => {
      const next = new Map(requested);
      const known = (n: string) => {
        const c = catalog.find((e) => e.name === n);
        if (!c) throw new AppError('E_VALIDATION', `${n} is not in the index`);
        return c;
      };
      if (op.op === 'install')
        for (const n of op.names) next.set(n, known(n).version);
      else if (op.op === 'remove')
        for (const n of op.names) {
          if (!next.has(n))
            throw new AppError('E_VALIDATION', `${n} is not installed`);
          next.delete(n);
        }
      else if (op.op === 'update')
        for (const n of op.names?.length ? op.names : [...next.keys()])
          if (next.has(n)) next.set(n, known(n).version);
      const { changes, downloadBytes } = diff(next);
      plan = {
        planSha256: sha(JSON.stringify(changes)),
        changes,
        downloadBytes,
        next,
      };
      return { planSha256: plan.planSha256, changes, downloadBytes };
    },
    'packages:apply': async ({ planSha256, downloadBytes }) => {
      const p = plan;
      if (
        !p ||
        p.planSha256 !== planSha256 ||
        p.downloadBytes !== downloadBytes
      )
        throw new AppError(
          'E_VALIDATION',
          'not the plan that was shown; plan again',
        );
      plan = null;
      for (let i = 1; i <= 10 && downloadBytes > 0; i++) {
        await new Promise((r) => setTimeout(r, tickMs));
        emit('packages:progress', {
          doneBytes: Math.round((downloadBytes * i) / 10),
          totalBytes: downloadBytes,
        });
      }
      requested = p.next;
      return { changes: p.changes, staleRevocation: false };
    },
    'packages:prune': () => ({ dirs: 0, blobs: 0, bytes: 0 }),
    'packages:set-enabled': ({ name, enabled }) => ({
      name,
      enabled: setEnabled(name, enabled),
    }),
  };
}
