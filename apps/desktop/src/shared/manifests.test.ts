import { describe, expect, it } from 'vitest';
import { manifests } from './manifests';
import { QRTZ_PAGE_PARTS, SIDEBAR_FOOT, sidebarItems } from '../shell/App';
import { DASH_ENABLED } from '../shell/subapps';

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

  it('the sidebar is echo, nano, ctrl, snap, then apps (plus icon, last of the list) and qrtz opts pinned to the bottom', () => {
    const sorted = manifests
      // ADR 0228: dash is behind DASH_ENABLED (off) in packaged builds.
      .filter((m) => m.id !== 'shell' && m.id !== 'gallery')
      .filter((m) => m.id !== 'dash' || DASH_ENABLED)
      .toSorted((a, b) => (a.order ?? 100) - (b.order ?? 100))
      .map((manifest) => ({ manifest }));
    const items = sidebarItems(sorted);
    expect(items.map((i) => i.label)).toEqual([
      'echo',
      'nano',
      'ctrl',
      'snap',
      'apps',
      'qrtz',
    ]);
    expect(items.slice(-2)).toEqual([
      { id: 'apps', label: 'apps', icon: 'plus', group: 'main' },
      { id: 'opts', label: 'qrtz', group: 'foot' },
    ]);
    expect(items.filter((i) => i.group === 'foot')).toHaveLength(1);
    expect(QRTZ_PAGE_PARTS).toEqual(new Set(['marketplace', 'settings']));
    expect([...SIDEBAR_FOOT]).toEqual([]);
    const withGallery = sidebarItems(
      manifests
        .filter((m) => m.id === 'gallery')
        .map((manifest) => ({ manifest })),
    );
    expect(withGallery.map((i) => i.id)).toEqual(['apps', 'opts']);
  });

  it('dash is off by default and holds the dash capabilities alone (ADR 0228)', () => {
    expect(DASH_ENABLED).toBe(false);
    const holders = manifests
      .filter((m) =>
        m.capabilities.some((c) => c === 'dash.read' || c === 'dash.manage'),
      )
      .map((m) => m.id);
    expect(holders).toEqual(['dash']);
  });

  it('keeps the shell ids apps and opts unique: a module with id or name apps/opts gets no row', () => {
    const mod = (id: string, name: string) => ({ manifest: { id, name } });
    const items = sidebarItems([
      mod('echo', 'echo'),
      mod('apps', 'evil'),
      mod('opts', 'opts'),
      mod('x1', 'apps'),
      mod('x2', ' OPTS '),
      mod('x3', 'Apps'),
      mod('x4', 'QRTZ'),
      mod('nano', 'nano'),
    ]);
    const ids = items.map((i) => i.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids).toEqual(['echo', 'nano', 'apps', 'opts']);
    const named = (l: string) =>
      items.filter((i) => i.label.trim().toLowerCase() === l);
    expect(named('apps')).toHaveLength(1);
    expect(named('qrtz')).toHaveLength(1);
    expect(items.slice(-2)).toEqual([
      { id: 'apps', label: 'apps', icon: 'plus', group: 'main' },
      { id: 'opts', label: 'qrtz', group: 'foot' },
    ]);
  });
});
