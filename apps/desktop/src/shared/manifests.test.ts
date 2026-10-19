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

  it('the sidebar is qrtz (no icon), then echo, nano, ctrl, snap: no apps or settings tabs at the bottom', () => {
    const sorted = manifests
      .filter((m) => m.id !== 'shell' && m.id !== 'gallery')
      .toSorted((a, b) => (a.order ?? 100) - (b.order ?? 100))
      .map((manifest) => ({ manifest }));
    const items = sidebarItems(sorted);
    expect(items.map((i) => i.label)).toEqual([
      'qrtz',
      'echo',
      'nano',
      'ctrl',
      'snap',
    ]);
    expect(items[0]).toEqual({ id: 'qrtz', label: 'qrtz' });
    expect(items.some((i) => i.group === 'foot')).toBe(false);
    expect(QRTZ_PAGE_PARTS).toEqual(new Set(['marketplace', 'settings']));
    expect([...SIDEBAR_FOOT]).toEqual(['gallery']);
  });
});
