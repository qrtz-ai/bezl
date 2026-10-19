import { describe, expect, it } from 'vitest';
import { manifests } from './manifests';

describe('manifests', () => {
  it('are unique, with no Home dashboard and a capability-free gallery', () => {
    const ids = manifests.map((m) => m.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(manifests.find((m) => m.id === 'home')).toBeUndefined();
    expect(manifests.find((m) => m.id === 'gallery')?.capabilities).toEqual([]);
  });

  it('sort echo first in the sidebar, then nano, ctrl and snap', () => {
    const order = manifests
      .filter((m) => m.order !== undefined)
      .toSorted((a, b) => (a.order ?? 0) - (b.order ?? 0))
      .map((m) => m.name);
    expect(order.slice(0, 4)).toEqual(['echo', 'nano', 'ctrl', 'snap']);
  });
});
