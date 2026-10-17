import { describe, expect, it } from 'vitest';
import { manifests } from './manifests';

describe('manifests', () => {
  it('are unique and least-privilege for home (dashboard: read, quick record, permission status)', () => {
    const ids = manifests.map((m) => m.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(manifests.find((m) => m.id === 'home')?.capabilities).toEqual([
      'app.info',
      'memo.read',
      'memo.record',
      'os.permissions',
    ]);
    expect(manifests.find((m) => m.id === 'gallery')?.capabilities).toEqual([]);
  });
});
