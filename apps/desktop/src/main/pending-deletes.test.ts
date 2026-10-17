import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { PendingDeletes } from './pending-deletes';

const dir = () => fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-pd-'));

describe('PendingDeletes', () => {
  it('deletes after the undo window and clears the tombstone', async () => {
    vi.useFakeTimers();
    const done: string[] = [];
    const pd = new PendingDeletes(dir(), async (id) => void done.push(id));
    pd.schedule('a', 6000);
    expect(pd.has('a')).toBe(true);
    await vi.advanceTimersByTimeAsync(5999);
    expect(done).toEqual([]);
    await vi.advanceTimersByTimeAsync(1);
    expect(done).toEqual(['a']);
    expect(pd.ids()).toEqual([]);
    vi.useRealTimers();
  });
  it('undo cancels; undo after completion reports too late', async () => {
    vi.useFakeTimers();
    const done: string[] = [];
    const pd = new PendingDeletes(dir(), async (id) => void done.push(id));
    pd.schedule('a', 1000);
    expect(pd.undo('a')).toBe(true);
    await vi.advanceTimersByTimeAsync(2000);
    expect(done).toEqual([]);
    pd.schedule('b', 10);
    await vi.advanceTimersByTimeAsync(20);
    expect(pd.undo('b')).toBe(false);
    vi.useRealTimers();
  });
  it('survives a quit: a new instance completes the tombstones on recover()', async () => {
    const d = dir();
    new PendingDeletes(d, async () => {}).schedule('x', 60_000);
    const done: string[] = [];
    const next = new PendingDeletes(d, async (id) => void done.push(id));
    expect(next.ids()).toEqual(['x']);
    await next.recover();
    expect(done).toEqual(['x']);
    expect(next.ids()).toEqual([]);
  });
  it('keeps the tombstone when the delete fails, and tolerates a corrupt file', async () => {
    const d = dir();
    const errs: string[] = [];
    const pd = new PendingDeletes(
      d,
      async () => {
        throw new Error('db busy');
      },
      (id) => errs.push(id),
    );
    pd.schedule('y', 60_000);
    await pd.recover();
    expect(errs).toEqual(['y']);
    expect(pd.ids()).toEqual(['y']);
    fs.writeFileSync(path.join(d, 'pending-deletes.json'), '{nope');
    expect(pd.ids()).toEqual([]);
  });
});
