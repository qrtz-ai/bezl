import { describe, expect, it } from 'vitest';
import { packagesHandlers } from './packages';

// The dev mock's package manager behaves like the real one where the marketplace can tell (ADR 0157/0161).
describe('mock package manager', () => {
  const make = () => {
    const progress: unknown[] = [];
    const on = new Map<string, boolean>();
    const h = packagesHandlers(
      (_e, p) => progress.push(p),
      (n, v) => {
        on.set(n, v);
        return v;
      },
      0,
    ) as Record<string, (req: unknown) => unknown>;
    const call = async (c: string, req?: unknown) => h[c]!(req);
    return { call, progress, on };
  };

  it('installs only the plan it showed, reporting progress, and removes', async () => {
    const m = make();
    const before = (await m.call('packages:list')) as {
      packages: { name: string }[];
    };
    expect(before.packages.map((p) => p.name)).not.toContain('ctrl');
    const plan = (await m.call('packages:plan', {
      op: 'install',
      names: ['ctrl'],
    })) as { planSha256: string; downloadBytes: number };
    expect(plan.downloadBytes).toBe(490_800_000);
    await expect(
      m.call('packages:apply', {
        planSha256: plan.planSha256,
        downloadBytes: 1,
      }),
    ).rejects.toThrow(/plan again/);
    await m.call('packages:apply', plan);
    expect(m.progress.at(-1)).toEqual({
      doneBytes: 490_800_000,
      totalBytes: 490_800_000,
    });
    const after = (await m.call('packages:list')) as {
      packages: { name: string }[];
    };
    expect(after.packages.map((p) => p.name)).toContain('model.laya');
    const rm = (await m.call('packages:plan', {
      op: 'remove',
      names: ['ctrl'],
    })) as { planSha256: string; downloadBytes: number; changes: unknown[] };
    expect(rm.downloadBytes).toBe(0);
    expect(rm.changes).toHaveLength(2);
    await m.call('packages:apply', rm);
  });

  it('switches through the shell config', async () => {
    const m = make();
    expect(
      await m.call('packages:set-enabled', { name: 'nano', enabled: false }),
    ).toEqual({ name: 'nano', enabled: false });
    expect(m.on.get('nano')).toBe(false);
  });
});
