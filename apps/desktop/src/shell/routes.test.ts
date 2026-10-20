import { describe, expect, it } from 'vitest';
import {
  hostOf,
  joinRoute,
  navTarget,
  pageOf,
  resolveRoute,
  splitRoute,
} from './routes';
import { homeRoute } from './routes';

const apps = ['note', 'talk'];

describe('main window routes (ADR 0091)', () => {
  it('valid routes resolve to themselves', () => {
    for (const p of [
      '/qrtz',
      '/qrtz/settings',
      '/qrtz/item/echo',
      '/note',
      '/talk',
      '/note/meetings/m1',
      '/note/meetings/m1/transcript',
      '/note/meetings/m1/summary',
      '/note/meetings/m1/notes',
    ])
      expect(resolveRoute(p, apps)).toBe(p);
  });

  it('the root and unknown routes land on the qrtz page', () => {
    for (const p of ['/', '', '/nope', '/nope/meetings/m1', '/hud', '/pill'])
      expect(resolveRoute(p, apps)).toBe('/qrtz');
  });

  it('malformed sub-paths fall back within their page', () => {
    expect(resolveRoute('/note/meetings', apps)).toBe('/note');
    expect(resolveRoute('/note/other/m1', apps)).toBe('/note');
    expect(resolveRoute('/note/meetings/m1/bogus', apps)).toBe(
      '/note/meetings/m1',
    );
    expect(resolveRoute('/note/meetings/m1/summary/extra', apps)).toBe('/note');
    expect(resolveRoute('/talk/anything', apps)).toBe('/talk');
    expect(resolveRoute('/note/meetings/m1/', apps)).toBe('/note/meetings/m1');
    expect(resolveRoute('/qrtz/x/y', apps)).toBe('/qrtz');
    expect(resolveRoute('/qrtz/item', apps)).toBe('/qrtz');
    expect(resolveRoute('/qrtz/item/Bad', apps)).toBe('/qrtz');
    expect(resolveRoute('/qrtz/settings/x', apps)).toBe('/qrtz');
  });

  it('splits and joins', () => {
    expect(splitRoute('/note/meetings/m1/summary')).toEqual({
      app: 'note',
      sub: 'meetings/m1/summary',
    });
    expect(splitRoute('/talk')).toEqual({ app: 'talk', sub: '' });
    expect(joinRoute('note', 'meetings/m1')).toBe('/note/meetings/m1');
    expect(joinRoute('talk')).toBe('/talk');
  });

  it('navigation requests map to routes, carrying the meeting and its tab', () => {
    expect(navTarget({ app: 'qrtz' }, apps)).toBe('/qrtz');
    expect(navTarget({ app: 'note', meetingId: 'm1' }, apps)).toBe(
      '/note/meetings/m1',
    );
    expect(
      navTarget({ app: 'note', meetingId: 'm1', tab: 'notes' }, apps),
    ).toBe('/note/meetings/m1/notes');
    // A meeting only means something to note; an unknown app goes to the qrtz page.
    expect(navTarget({ app: 'talk', meetingId: 'm1' }, apps)).toBe('/talk');
    expect(navTarget({ app: 'gone' }, apps)).toBe('/qrtz');
  });
});

describe('the qrtz page (app manager + its settings) and module settings routes', () => {
  it('redirects the old marketplace, module manager and settings routes to the qrtz page', () => {
    expect(resolveRoute('/marketplace', apps)).toBe('/qrtz');
    expect(resolveRoute('/marketplace/installed', apps)).toBe('/qrtz');
    expect(resolveRoute('/marketplace/item/echo', apps)).toBe(
      '/qrtz/item/echo',
    );
    expect(resolveRoute('/marketplace/item/model.laya', apps)).toBe(
      '/qrtz/item/model.laya',
    );
    expect(resolveRoute('/marketplace/item/Bad', apps)).toBe('/qrtz');
    expect(resolveRoute('/settings', apps)).toBe('/qrtz/settings');
    expect(resolveRoute('/settings/settings', apps)).toBe('/qrtz/settings');
  });

  it("keeps each module's own settings page", () => {
    const all = ['note', 'talk', 'ctrl', 'snap'];
    for (const p of [
      '/talk/settings',
      '/note/settings',
      '/ctrl/settings',
      '/snap/settings',
    ])
      expect(resolveRoute(p, all)).toBe(p);
  });

  it('settings requests open the right settings page; old app ids and an item open the qrtz page', () => {
    expect(navTarget({ app: 'talk', settings: true }, apps)).toBe(
      '/talk/settings',
    );
    expect(navTarget({ app: 'qrtz', settings: true }, apps)).toBe(
      '/qrtz/settings',
    );
    expect(navTarget({ app: 'settings' }, apps)).toBe('/qrtz/settings');
    expect(navTarget({ app: 'settings', settings: true }, apps)).toBe(
      '/qrtz/settings',
    );
    expect(navTarget({ app: 'marketplace' }, apps)).toBe('/qrtz');
    expect(navTarget({ app: 'qrtz', item: 'echo' }, apps)).toBe(
      '/qrtz/item/echo',
    );
    expect(navTarget({ app: 'qrtz', item: '../x' }, apps)).toBe('/qrtz');
  });

  it('the qrtz page is two sub apps: the app manager, and the settings at /qrtz/settings', () => {
    expect(hostOf('qrtz', '')).toEqual({ host: 'marketplace', sub: '' });
    expect(hostOf('qrtz', 'item/nano')).toEqual({
      host: 'marketplace',
      sub: 'item/nano',
    });
    expect(hostOf('qrtz', 'settings')).toEqual({ host: 'settings', sub: '' });
    expect(hostOf('note', 'meetings/m1')).toEqual({
      host: 'note',
      sub: 'meetings/m1',
    });
    expect(pageOf('marketplace', '')).toBe('/qrtz');
    expect(pageOf('marketplace', 'item/nano')).toBe('/qrtz/item/nano');
    expect(pageOf('settings', '')).toBe('/qrtz/settings');
    expect(pageOf('note', 'meetings/m1')).toBe('/note/meetings/m1');
  });
});

describe('the default page (QRT-269)', () => {
  const apps = ['talk', 'note', 'ctrl'];
  it('is the top sidebar app, whatever it is', () => {
    expect(homeRoute(['note', 'talk'])).toBe('/note');
    expect(homeRoute(['ctrl'])).toBe('/ctrl');
  });
  it('is the apps page when no app is on', () => {
    expect(homeRoute([])).toBe('/qrtz');
    expect(resolveRoute('/', [], homeRoute([]))).toBe('/qrtz');
  });
  it('catches /, unknown and switched-off apps; explicit qrtz routes stay', () => {
    const home = homeRoute(['note', 'talk']);
    expect(resolveRoute('/', apps, home)).toBe('/note');
    expect(resolveRoute('/nope/x', apps, home)).toBe('/note');
    expect(resolveRoute('/snap', apps, home)).toBe('/note');
    expect(resolveRoute('/qrtz', apps, home)).toBe('/qrtz');
    expect(resolveRoute('/settings', apps, home)).toBe('/qrtz/settings');
    expect(resolveRoute('/marketplace', apps, home)).toBe('/qrtz');
    expect(navTarget({ app: 'snap' }, apps, home)).toBe('/note');
  });
});
