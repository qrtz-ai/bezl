import { describe, expect, it } from 'vitest';
import { manifests } from './manifests';

describe('manifests', () => {
  it('are unique, with no Home dashboard and a capability-free gallery', () => {
    const ids = manifests.map((m) => m.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(manifests.find((m) => m.id === 'home')).toBeUndefined();
    expect(manifests.find((m) => m.id === 'gallery')?.capabilities).toEqual([]);
  });
});
