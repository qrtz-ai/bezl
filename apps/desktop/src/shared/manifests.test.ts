import { describe, expect, it } from 'vitest';
import { manifests } from './manifests';
import { QRTZ_PAGE_PARTS, SIDEBAR_FOOT, sidebarItems } from '../shell/App';

describe('manifests', () => {
  it('are unique, with no Home dashboard and a capability-free gallery', () => {
    const ids = manifests.map((m) => m.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(manifests.find((m) => m.id === 'home')).toBeUndefined();
    expect(manifests.find((m) => m.id === 'gallery')?.capabilities).toEqual([]);
  });

  it('sort echo first among the apps, then nano, ctrl and snap', () => {
    const order = manifests
      .filter((m) => m.order !== undefined)
      .toSorted((a, b) => (a.order ?? 0) - (b.order ?? 0))
      .map((m) => m.name);
    expect(order.slice(0, 4)).toEqual(['echo', 'nano', 'ctrl', 'snap']);
  });

  it('the sidebar is echo, nano, ctrl, snap, then apps and opts (no icons) pinned to the bottom', () => {
    const sorted = manifests
      .filter((m) => m.id !== 'shell' && m.id !== 'gallery')
      .toSorted((a, b) => (a.order ?? 100) - (b.order ?? 100))
      .map((manifest) => ({ manifest }));
    const items = sidebarItems(sorted);
    expect(items.map((i) => i.label)).toEqual([
      'echo',
      'nano',
      'ctrl',
      'snap',
      'apps',
      'opts',
    ]);
    expect(items.slice(-2)).toEqual([
      { id: 'apps', label: 'apps', group: 'foot' },
      { id: 'opts', label: 'opts', group: 'foot' },
    ]);
    expect(items.filter((i) => i.group === 'foot')).toHaveLength(2);
    expect(QRTZ_PAGE_PARTS).toEqual(new Set(['marketplace', 'settings']));
    expect([...SIDEBAR_FOOT]).toEqual(['gallery']);
  });
});
