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
 *   /<app id>                               a sub app; `/` and anything unknown → the first registry app
 *   /note/meetings/:meetingId/:tab?         a meeting; tab ∈ transcript (default) | summary | notes
 *   /talk|note|ctrl|snap/settings               a module's own settings page, in its view (ADR 0161)
 *   /marketplace/installed                   the module manager
 *   /marketplace/item/:name                  an extension's detail page
 * Other sub apps have one view each, so any deeper path goes to `/<app id>`.
 */
const MODULE_VIEWS_WITH_SETTINGS = ['talk', 'note', 'ctrl', 'snap'];
const MEETING = '/note/meetings/:meetingId/:tab?';

/** The canonical path for `pathname`; equal to it when it's already a valid route. */
export function resolveRoute(
  pathname: string,
  apps: readonly string[],
): string {
  const fallback = `/${apps[0] ?? ''}`;
  const [app, ...rest] = pathname.split('/').filter(Boolean);
  if (!app || !apps.includes(app)) return fallback;
  if (rest.length === 0) return `/${app}`;
  if (
    rest.length === 1 &&
    rest[0] === SETTINGS_PATH &&
    MODULE_VIEWS_WITH_SETTINGS.includes(app)
  )
    return `/${app}/${SETTINGS_PATH}`;
  if (app === 'marketplace') {
    if (rest.length === 1 && rest[0] === 'installed')
      return '/marketplace/installed';
    if (
      rest.length === 2 &&
      rest[0] === 'item' &&
      /^[a-z][a-z0-9.-]{0,63}$/.test(rest[1] ?? '')
    )
      return `/marketplace/item/${rest[1]}`;
    return '/marketplace';
  }
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

/** Where a navigation request (palette, tray, notification, a sub app) goes. */
export function navTarget(r: NavRequest, apps: readonly string[]): string {
  if (!apps.includes(r.app)) return resolveRoute('/', apps);
  if (r.app === 'note' && r.meetingId)
    return joinRoute('note', meetingPath(r.meetingId, r.tab));
  if (r.settings) return resolveRoute(joinRoute(r.app, SETTINGS_PATH), apps);
  return joinRoute(r.app);
}
