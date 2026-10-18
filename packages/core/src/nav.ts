/**
 * In-renderer navigation requests between shell-owned UI (palette), sub apps and the shell router (ADR 0091).
 * Sub apps never import each other; they ask the shell to switch apps and optionally open a meeting (and one
 * of its tabs). The shell turns a request into a route; the note view reads the meeting from its route.
 */
export interface NavRequest {
  app: string;
  meetingId?: string;
  /** With `meetingId`: which meeting tab to open (default transcript). */
  tab?: MeetingTab;
}

export const MEETING_TABS = ['transcript', 'summary', 'notes'] as const;
export type MeetingTab = (typeof MEETING_TABS)[number];
/** The transcript is front and center: the default view during and after a recording. */
export const DEFAULT_MEETING_TAB: MeetingTab = 'transcript';
export const isMeetingTab = (s: string | undefined): s is MeetingTab =>
  (MEETING_TABS as readonly string[]).includes(s ?? '');

/** The note app's sub-path for a meeting: `meetings/<id>`, plus `/<tab>` unless it's the default. */
export function meetingPath(id: string, tab?: MeetingTab): string {
  const t = tab && tab !== DEFAULT_MEETING_TAB ? `/${tab}` : '';
  return `meetings/${encodeURIComponent(id)}${t}`;
}

/** Inverse of `meetingPath`: anything else (the list, an unknown tab) is no meeting / the default tab. */
export function parseMeetingPath(sub: string): {
  meetingId: string | null;
  tab: MeetingTab;
} {
  const [kind, id, tab] = sub.split('/').filter(Boolean);
  if (kind !== 'meetings' || !id)
    return { meetingId: null, tab: DEFAULT_MEETING_TAB };
  return {
    meetingId: decodeURIComponent(id),
    tab: isMeetingTab(tab) ? tab : DEFAULT_MEETING_TAB,
  };
}

type Listener = (r: NavRequest) => void;
const listeners = new Set<Listener>();

export function requestNavigate(r: NavRequest): void {
  for (const l of listeners) l(r);
}

export function onNavigateRequest(l: Listener): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}
