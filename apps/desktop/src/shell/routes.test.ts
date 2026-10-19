import { describe, expect, it } from 'vitest';
import { joinRoute, navTarget, resolveRoute, splitRoute } from './routes';

const apps = ['note', 'talk', 'settings'];

describe('main window routes (ADR 0091)', () => {
  it('valid routes resolve to themselves', () => {
    for (const p of [
      '/note',
      '/talk',
      '/settings',
      '/note/meetings/m1',
      '/note/meetings/m1/transcript',
      '/note/meetings/m1/summary',
      '/note/meetings/m1/notes',
    ])
      expect(resolveRoute(p, apps)).toBe(p);
  });

  it('the root and unknown routes go to the first registry app', () => {
    for (const p of ['/', '', '/nope', '/nope/meetings/m1', '/hud', '/pill'])
      expect(resolveRoute(p, apps)).toBe('/note');
  });

  it('malformed sub-paths fall back within their app', () => {
    expect(resolveRoute('/note/meetings', apps)).toBe('/note');
    expect(resolveRoute('/note/other/m1', apps)).toBe('/note');
    expect(resolveRoute('/note/meetings/m1/bogus', apps)).toBe(
      '/note/meetings/m1',
    );
    expect(resolveRoute('/note/meetings/m1/summary/extra', apps)).toBe('/note');
    expect(resolveRoute('/talk/anything', apps)).toBe('/talk');
    expect(resolveRoute('/settings/x/y', apps)).toBe('/settings');
    expect(resolveRoute('/note/meetings/m1/', apps)).toBe('/note/meetings/m1');
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
    expect(navTarget({ app: 'settings' }, apps)).toBe('/settings');
    expect(navTarget({ app: 'note', meetingId: 'm1' }, apps)).toBe(
      '/note/meetings/m1',
    );
    expect(
      navTarget({ app: 'note', meetingId: 'm1', tab: 'notes' }, apps),
    ).toBe('/note/meetings/m1/notes');
    // A meeting only means something to note; an unknown app goes to the default.
    expect(navTarget({ app: 'talk', meetingId: 'm1' }, apps)).toBe('/talk');
    expect(navTarget({ app: 'gone' }, apps)).toBe('/note');
  });
});

describe('marketplace and module settings routes (ADR 0161)', () => {
  const all = ['note', 'talk', 'ctrl', 'marketplace', 'settings'];
  it('keeps the marketplace pages and module settings pages', () => {
    for (const p of [
      '/marketplace',
      '/marketplace/installed',
      '/marketplace/item/echo',
      '/marketplace/item/model.laya',
      '/talk/settings',
      '/note/settings',
      '/ctrl/settings',
    ])
      expect(resolveRoute(p, all)).toBe(p);
    expect(resolveRoute('/marketplace/item', all)).toBe('/marketplace');
    expect(resolveRoute('/marketplace/item/Bad', all)).toBe('/marketplace');
    expect(resolveRoute('/settings/settings', all)).toBe('/settings');
  });
  it('a settings request opens the module view at its settings page', () => {
    expect(navTarget({ app: 'talk', settings: true }, all)).toBe(
      '/talk/settings',
    );
    expect(navTarget({ app: 'settings', settings: true }, all)).toBe(
      '/settings',
    );
  });
});
