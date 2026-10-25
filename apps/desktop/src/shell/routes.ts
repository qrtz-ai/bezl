import { matchPath } from 'react-router';
import {
  SETTINGS_PATH,
  isMeetingTab,
  meetingPath,
  type NavRequest,
} from '@qrtz/core';

/**
 * The main window's routes (ADR 0091), under a hash router (`index.html#/…`; the Tauri custom protocol has no
 * server to answer deep paths):
 *   /qrtz                                   the qrtz page: the app manager (installed, available); the default only when no app is on
 *   /qrtz/item/:name                        the qrtz page with that app's detail modal open
 *   /qrtz/settings                          qrtz's own (global) settings, behind the qrtz page's gear
 *   /<app id>                               a sub app; `/` and anything unknown → the top sidebar app (homeRoute)
 *   /note/meetings/:meetingId/:tab?         a meeting; tab ∈ transcript (default) | summary | notes
 *   /talk|note|ctrl|snap/settings           a module's view with its opts modal open (ADR 0161)
 * The old /marketplace… and /settings routes redirect to the qrtz page. Other sub apps have one view each, so
 * any deeper path goes to `/<app id>`.
 */
const MODULE_VIEWS_WITH_SETTINGS = ['talk', 'note', 'ctrl', 'snap'];
const MEETING = '/note/meetings/:meetingId/:tab?';
const ITEM = /^[a-z][a-z0-9.-]{0,63}$/;

/** The qrtz page's id in the sidebar and the route. */
export const QRTZ = 'qrtz';
/**
 * The two sub apps the qrtz page is made of, each with its own scoped client: the app manager (`marketplace`:
 * packages, switches, permissions, models) and qrtz's settings (`settings`: config writes, backups, logs,
 * updates). One page for the user; two least-privilege IPC identities, unchanged from before.
 */
export const QRTZ_PARTS = { manager: 'marketplace', settings: 'settings' };
const isPart = (id: string) => id === 'marketplace' || id === 'settings';

/** The qrtz page's canonical path for its sub-path (`''`, `settings`, `item/<name>`). */
function qrtzRoute(rest: readonly string[]): string {
  if (rest.length === 1 && rest[0] === SETTINGS_PATH) return '/qrtz/settings';
  if (rest.length === 2 && rest[0] === 'item' && ITEM.test(rest[1] ?? ''))
    return `/qrtz/item/${rest[1]}`;
  return '/qrtz';
}

/**
 * The default page: the top app in the sidebar (`navApps` in sidebar order), or the apps page (/qrtz, where
 * apps are turned on) when no app is on.
 */
export const homeRoute = (navApps: readonly string[]): string =>
  navApps[0] ? `/${navApps[0]}` : '/qrtz';

/**
 * The canonical path for `pathname`; equal to it when it's already a valid route. `apps` are the sub apps the
 * sidebar shows below qrtz (the qrtz page itself is always there).
 */
export function resolveRoute(
  pathname: string,
  apps: readonly string[],
  home = '/qrtz',
): string {
  const [app, ...rest] = pathname.split('/').filter(Boolean);
  if (app === QRTZ) return qrtzRoute(rest);
  // Redirects from before the qrtz page: the marketplace and the module manager are its body, an item its modal.
  if (app === 'marketplace')
    return rest[0] === 'item' ? qrtzRoute(rest) : '/qrtz';
  if (app === 'settings') return '/qrtz/settings';
  if (isPart(app ?? '')) return '/qrtz';
  // `/`, an unknown app and an app that was just switched off land on the default page.
  if (!app || !apps.includes(app)) return home;
  if (rest.length === 0) return `/${app}`;
  if (
    rest.length === 1 &&
    rest[0] === SETTINGS_PATH &&
    MODULE_VIEWS_WITH_SETTINGS.includes(app)
  )
    return `/${app}/${SETTINGS_PATH}`;
  if (app === 'note') {
    const m = matchPath(MEETING, pathname);
    if (m?.params.meetingId)
      return isMeetingTab(m.params.tab) || m.params.tab === undefined
        ? pathname.replace(/\/+$/, '')
        : `/note/meetings/${encodeURIComponent(m.params.meetingId)}`;
  }
  return `/${app}`;
}

/** `/note/meetings/x/summary` → { app: 'note', sub: 'meetings/x/summary' } (input already resolved). */
export function splitRoute(path: string): { app: string; sub: string } {
  const [app = '', ...rest] = path.split('/').filter(Boolean);
  return { app, sub: rest.join('/') };
}

export const joinRoute = (app: string, sub = '') =>
  sub ? `/${app}/${sub}` : `/${app}`;

/**
 * Which sub app shows a resolved page path, and its own sub-path: the qrtz page's settings view is the settings
 * sub app; the rest of the qrtz page (the list and an item's modal) is the app manager.
 */
export function hostOf(
  page: string,
  sub: string,
): { host: string; sub: string } {
  if (page !== QRTZ) return { host: page, sub };
  return sub === SETTINGS_PATH
    ? { host: QRTZ_PARTS.settings, sub: '' }
    : { host: QRTZ_PARTS.manager, sub };
}

/** The page path a sub app's own sub-path is at (the inverse of `hostOf`). */
export function pageOf(host: string, sub: string): string {
  if (host === QRTZ_PARTS.settings) return '/qrtz/settings';
  if (host === QRTZ_PARTS.manager)
    return qrtzRoute(sub.split('/').filter(Boolean));
  return joinRoute(host, sub);
}

/** Where a navigation request (palette, tray, notification, a sub app) goes. */
export function navTarget(
  r: NavRequest,
  apps: readonly string[],
  home = '/qrtz',
): string {
  if (r.app === QRTZ || r.app === 'marketplace') {
    if (r.settings) return '/qrtz/settings';
    return r.item ? qrtzRoute(['item', r.item]) : '/qrtz';
  }
  if (r.app === 'settings') return '/qrtz/settings';
  if (!apps.includes(r.app)) return home;
  if (r.app === 'note' && r.meetingId)
    return joinRoute('note', meetingPath(r.meetingId, r.tab));
  if (r.settings) return resolveRoute(joinRoute(r.app, SETTINGS_PATH), apps);
  return joinRoute(r.app);
}

/** The sidebar ids of the qrtz page's two halves: apps (/qrtz, the app manager) and opts (/qrtz/settings). */
export const APPS = 'apps';
export const OPTS = 'opts';
/**
 * Shell-owned sidebar ids and labels. A module whose id or display name is one of these (any case, any padding)
 * gets no sidebar row and no palette entry, so no module can pose as, or collide with, the apps or opts rows.
 */
export const RESERVED_NAV: ReadonlySet<string> = new Set([APPS, OPTS, 'qrtz']); // 'qrtz' is opts' sidebar label
export const isReservedNav = (s: string) =>
  RESERVED_NAV.has(s.trim().toLowerCase());
