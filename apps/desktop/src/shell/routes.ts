import { matchPath } from 'react-router';
import { isMeetingTab, meetingPath, type NavRequest } from '@qrtz/core';

/**
 * The main window's routes (ADR 0091), under a hash router (`index.html#/…`; the Tauri custom protocol has no
 * server to answer deep paths):
 *   /<app id>                               a sub app; `/` and anything unknown → the first registry app
 *   /note/meetings/:meetingId/:tab?         a meeting; tab ∈ transcript (default) | summary | notes
 * Other sub apps have one view each, so any deeper path goes to `/<app id>`.
 */
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
  return joinRoute(r.app);
}
